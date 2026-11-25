//! Bounded subscriptions and virtualized log output for a container or Compose project.

use marsa_ui::gpui_component::scroll::ScrollableElement as _;

use crate::screens::common::message;
use marsa_ui::{
    gui_keys as gui,
    tokens::typography::{Family, ui_size},
};
use std::{collections::VecDeque, io::Write as _, path::Path, sync::Arc};

use marsa_client::docker::{ContainerSummary, LogLine, LogSource};
use marsa_ui::{
    ColorToken, Locale,
    direction::{isolate_technical_text, row},
    gpui::{
        AnyElement, AppContext as _, ClipboardItem, Context, Div, Entity, InteractiveElement as _,
        IntoElement, ParentElement as _, ScrollStrategy, Styled as _, Subscription, Task,
        UniformListScrollHandle, Window, div, px, uniform_list,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    preferences::{LogSettings, TimeFormat},
};

use crate::screens::common::{ScreenContext, label, log_time};

use super::{ContainerView, ansi, color, name};

pub(super) const MAX_SOURCES: usize = 32;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct LogsState {
    rows: Arc<VecDeque<Line>>,
    settings: LogSettings,
    hide_timestamps: bool,
    bytes: usize,
    paused: Option<Arc<VecDeque<Line>>>,
    query: String,
    matching: Arc<Vec<usize>>,
    indexed_rows: Arc<VecDeque<Line>>,
    search_dirty: bool,
    match_cursor: usize,
    source: Option<LogSource>,
    scroll: UniformListScrollHandle,
    wrap: bool,
    failed: bool,
    source_limit: bool,
    open_streams: usize,
    closed: bool,
    save_failed: bool,
    pub(super) search_input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    search_task: Option<Task<()>>,
}

#[derive(Clone)]
struct Line {
    container: Arc<str>,
    content: LogLine,
}

impl LogsState {
    pub(super) fn new(settings: LogSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub(super) fn settings(&self) -> LogSettings {
        self.settings
    }

    pub(super) fn set_settings(&mut self, settings: LogSettings, cx: &mut Context<ContainerView>) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        let limit = settings.buffer.container_lines();
        let truncated = self.trim();
        let paused_truncated = if let Some(paused) = &mut self.paused
            && paused.len() > limit
        {
            let excess = paused.len() - limit;
            Arc::make_mut(paused).drain(..excess);
            true
        } else {
            false
        };
        if truncated || paused_truncated {
            self.indexed_rows = Arc::default();
            self.refilter(cx);
        } else {
            cx.notify();
        }
    }

    fn timestamp(&self, line: &LogLine) -> Option<String> {
        if self.hide_timestamps {
            return None;
        }
        line.timestamp
            .map(|timestamp| timestamp.format(self.settings.local_timestamps))
    }

    pub(super) fn set_source_limit(&mut self, limited: bool) {
        self.source_limit = limited;
    }

    pub(super) fn set_stream_count(&mut self, count: usize) {
        self.open_streams = count;
    }

    pub(super) fn prepare_search(
        &mut self,
        window: &mut Window,
        locale: marsa_ui::Locale,
        cx: &mut Context<ContainerView>,
    ) {
        if self.search_input.is_some() {
            return;
        }
        let placeholder = gui::GUI_DESKTOP_SEARCH_LOGS.text(locale);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        self.subscription = Some(cx.subscribe(&input, |view, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.logs.query = input.read(cx).value().to_lowercase();
                view.logs.match_cursor = 0;
                view.logs.refilter(cx);
            }
        }));
        self.search_input = Some(input);
    }

    fn refilter(&mut self, cx: &mut Context<ContainerView>) {
        self.search_task = None;
        self.matching = Arc::new(Vec::new());
        self.update_matches(cx);
    }

    fn update_matches(&mut self, cx: &mut Context<ContainerView>) {
        if self.search_task.is_some() {
            self.search_dirty = true;
            return;
        }
        self.search_dirty = false;
        if self.query.is_empty() && self.source.is_none() {
            self.matching = Arc::new(Vec::new());
            cx.notify();
            return;
        }
        let rows = self.paused.as_ref().unwrap_or(&self.rows).clone();
        let query = self.query.clone();
        let source = self.source;
        let scan = cx.background_executor().spawn(async move {
            let matches = rows
                .iter()
                .enumerate()
                .filter_map(|(index, line)| {
                    (source.is_none_or(|source| source == line.content.source)
                        && (query.is_empty()
                            || ansi::plain(&line.content.text)
                                .to_lowercase()
                                .contains(&query)))
                    .then_some(index)
                })
                .collect::<Vec<_>>();
            (rows, matches)
        });
        self.search_task = Some(cx.spawn(async move |view, cx| {
            let (rows, matches) = scan.await;
            let _ = view.update(cx, |view, cx| {
                view.logs.match_cursor =
                    view.logs.match_cursor.min(matches.len().saturating_sub(1));
                view.logs.indexed_rows = rows;
                view.logs.matching = Arc::new(matches);
                view.logs.search_task = None;
                if view.logs.search_dirty {
                    view.logs.update_matches(cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn append(&mut self, container: &Arc<str>, batch: Vec<LogLine>) {
        let rows = Arc::make_mut(&mut self.rows);
        for line in batch {
            self.bytes = self.bytes.saturating_add(line.retained_bytes());
            rows.push_back(Line {
                container: Arc::clone(container),
                content: line,
            });
        }
        self.trim();
    }

    fn trim(&mut self) -> bool {
        let limit = self.settings.buffer.container_lines();
        if self.rows.len() <= limit && self.bytes <= MAX_BYTES {
            return false;
        }
        let rows = Arc::make_mut(&mut self.rows);
        while rows.len() > limit || self.bytes > MAX_BYTES {
            let Some(line) = rows.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(line.content.retained_bytes());
        }
        true
    }
}

pub(super) fn follow(
    mut stream: crate::services::Batches<LogLine>,
    container: Arc<str>,
    cx: &Context<ContainerView>,
) -> Task<()> {
    cx.spawn(async move |view, cx| {
        while let Some(batch) = stream.next().await {
            if view
                .update(cx, |view, cx| {
                    match batch {
                        Ok(lines) => view.logs.append(&container, lines),
                        Err(_) => view.logs.failed = true,
                    }
                    if view.logs.paused.is_none() {
                        view.logs.update_matches(cx);
                    } else {
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }
        }
        let _ = view.update(cx, |view, cx| {
            view.logs.open_streams = view.logs.open_streams.saturating_sub(1);
            view.logs.closed = view.logs.open_streams == 0;
            cx.notify();
        });
    })
}

pub(super) fn render(
    view: &ContainerView,
    entry: &ContainerSummary,
    screen: &ScreenContext<'_>,
    cx: &mut Context<ContainerView>,
) -> AnyElement {
    let palette = screen.palette;
    let log = &view.logs;
    let mut toolbar = row(cx).gap_2().items_center().flex_wrap();
    if let Some(input) = log.search_input.as_ref() {
        toolbar = toolbar.child(
            div().w(px(232.)).child(
                Input::new(input)
                    .cleanable(true)
                    .prefix(IconName::Search)
                    .aria_label(gui::GUI_DESKTOP_SEARCH_LOGS.text(screen.locale)),
            ),
        );
    }
    let matches = log.matching.len();
    let filtering = !log.query.is_empty() || log.source.is_some();
    {
        toolbar = toolbar
            .child(
                Button::new("previous-log-match")
                    .outline()
                    .icon(IconName::ChevronLeft)
                    .accessibility_label(message(screen.locale, gui::GUI_DESKTOP_PREVIOUS_MATCH))
                    .disabled(matches == 0)
                    .on_click(cx.listener(|view, _, _, cx| move_match(view, false, cx))),
            )
            .child(
                Button::new("next-log-match")
                    .outline()
                    .icon(IconName::ChevronRight)
                    .accessibility_label(message(screen.locale, gui::GUI_DESKTOP_NEXT_MATCH))
                    .disabled(matches == 0)
                    .on_click(cx.listener(|view, _, _, cx| move_match(view, true, cx))),
            )
            .child(div().text_size(ui_size(11.)).child(if !filtering {
                String::new()
            } else if matches == 0 {
                "0 / 0".to_owned()
            } else {
                format!("{} / {matches}", log.match_cursor + 1)
            }));
    }
    let weak = cx.entity().downgrade();
    let locale = screen.locale;
    toolbar = toolbar
        .child(
            Button::new("log-source-filter")
                .outline()
                .label(source_label(log.source).text(locale))
                .icon(IconName::ChevronDown)
                .dropdown_menu(move |menu, _, _| {
                    [
                        None,
                        Some(LogSource::Stdout),
                        Some(LogSource::Stderr),
                        Some(LogSource::Terminal),
                    ]
                    .into_iter()
                    .fold(menu, |menu, source| {
                        let view = weak.clone();
                        menu.item(
                            PopupMenuItem::new(source_label(source).text(locale)).on_click(
                                move |_, _, cx| {
                                    let _ = view.update(cx, |view, cx| {
                                        view.logs.source = source;
                                        view.logs.refilter(cx);
                                    });
                                },
                            ),
                        )
                    })
                }),
        )
        .child(
            Button::new("log-timestamps")
                .outline()
                .selected(!log.hide_timestamps)
                .label(gui::GUI_DESKTOP_CONTAINERS_LOG_TIMESTAMPS.text(locale))
                .on_click(cx.listener(|view, _, _, cx| {
                    view.logs.hide_timestamps = !view.logs.hide_timestamps;
                    cx.notify();
                })),
        )
        .child(div().flex_1())
        .child(
            Button::new("log-follow")
                .primary()
                .selected(log.paused.is_none())
                .label(gui::GUI_DESKTOP_CONTAINERS_LOG_FOLLOW_LIVE.text(locale))
                .on_click(cx.listener(|view, _, _, cx| {
                    if view.logs.paused.take().is_some() {
                        view.logs.refilter(cx);
                    }
                })),
        )
        .child(
            Button::new("log-pause")
                .outline()
                .icon(if log.paused.is_some() {
                    IconName::Play
                } else {
                    IconName::Pause
                })
                .accessibility_label(if log.paused.is_some() {
                    gui::GUI_DESKTOP_FOLLOW.text(locale)
                } else {
                    gui::GUI_DESKTOP_PAUSE.text(locale)
                })
                .on_click(cx.listener(|view, _, _, cx| {
                    view.logs.paused = view.logs.paused.is_none().then(|| view.logs.rows.clone());
                    view.logs.refilter(cx);
                })),
        );

    let sources = entry.compose_project().map_or_else(
        || isolate_technical_text(name(entry)),
        isolate_technical_text,
    );
    let mut source_bar = row(cx)
        .items_center()
        .flex_wrap()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family("IBM Plex Mono")
                .text_size(ui_size(12.))
                .child(sources),
        )
        .child(div().flex_1())
        .child(
            Button::new("log-scope")
                .ghost()
                .small()
                .label(if view.combined_logs {
                    message(screen.locale, gui::GUI_DESKTOP_SELECTED_CONTAINER)
                } else {
                    message(screen.locale, gui::GUI_DESKTOP_COMPOSE_PROJECT_LOGS)
                })
                .disabled(entry.compose_project().is_none())
                .on_click(cx.listener(|view, _, _, cx| {
                    view.combined_logs = !view.combined_logs;
                    view.set_tab(super::Tab::Logs, cx);
                })),
        );

    if log.source_limit {
        source_bar = source_bar.child(div().text_size(ui_size(11.)).child(message(
            screen.locale,
            gui::GUI_DESKTOP_SHOWING_LOGS_FROM_UP_TO_32_PROJECT_CONTAINERS,
        )));
    }

    let snapshot = if filtering {
        log.indexed_rows.clone()
    } else {
        log.paused.as_ref().unwrap_or(&log.rows).clone()
    };
    let row_count = if filtering { matches } else { snapshot.len() };
    let selected = log.match_cursor;
    let wrap = log.wrap;
    let indexes = log.matching.clone();
    let timestamps = (!log.hide_timestamps).then_some((
        screen.locale,
        screen.display.time_format,
        log.settings.local_timestamps,
    ));

    if log.paused.is_none() && !filtering && row_count > 0 {
        log.scroll
            .scroll_to_item(row_count - 1, ScrollStrategy::Nearest);
    }

    let output = div()
        .flex()
        .flex_col()
        .min_h_0()
        .flex_1()
        .rounded_lg()
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgCode))
        .overflow_hidden()
        .child(
            row(cx)
                .items_center()
                .flex_wrap()
                .gap_2()
                .px_3()
                .py_2()
                .bg(color(palette, ColorToken::BgSubtle))
                .child(
                    Button::new("log-wrap")
                        .ghost()
                        .small()
                        .label(if wrap {
                            message(screen.locale, gui::GUI_DESKTOP_UNWRAP_LINES)
                        } else {
                            message(screen.locale, gui::GUI_DESKTOP_WRAP_LINES)
                        })
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.logs.wrap = !view.logs.wrap;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("copy-container-logs")
                        .ghost()
                        .small()
                        .icon(IconName::Copy)
                        .accessibility_label(message(
                            screen.locale,
                            gui::GUI_DESKTOP_COPY_BUFFERED_LOGS,
                        ))
                        .disabled(snapshot.is_empty())
                        .on_click(cx.listener(|view, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(export(&view.logs)));
                        })),
                )
                .child(
                    Button::new("save-container-logs")
                        .ghost()
                        .small()
                        .icon(Icon::empty().path("icons/download.svg"))
                        .accessibility_label(message(
                            screen.locale,
                            gui::GUI_DESKTOP_SAVE_BUFFERED_LOGS,
                        ))
                        .disabled(snapshot.is_empty())
                        .on_click(cx.listener(|view, _, _, cx| save(view, cx))),
                ),
        );

    let mut output = if row_count == 0 {
        let (en, ar) = if log.failed {
            ("Couldn’t read container logs.", "تعذّرت قراءة سجلات الحاوية.")
        } else if filtering {
            ("No matching log lines.", "لا توجد أسطر سجلات مطابقة.")
        } else if log.closed {
            (
                "No log lines were returned.",
                "لم تُرجع الحاوية أي أسطر سجلات.",
            )
        } else {
            ("Waiting for log output…", "جارٍ انتظار مخرجات السجل…")
        };
        output.child(div().p_4().child(label(screen, en, ar)))
    } else if wrap {
        // Wrapped rows have variable heights, so use a bounded scroll container.
        let start = row_count.saturating_sub(500);
        output.child(
            div()
                .id("container-logs-wrapped")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .children((start..row_count).filter_map(|index| {
                    let source = if filtering {
                        indexes.get(index).copied()?
                    } else {
                        index
                    };
                    snapshot.get(source).map(|line| {
                        line_element(
                            line,
                            palette,
                            filtering && selected == index,
                            true,
                            timestamps,
                        )
                    })
                })),
        )
    } else {
        let list = uniform_list("container-log-buffer", row_count, move |range, _, _| {
            range
                .filter_map(|index| {
                    let source = if filtering {
                        indexes.get(index).copied()?
                    } else {
                        index
                    };
                    snapshot.get(source).map(|line| {
                        line_element(
                            line,
                            palette,
                            filtering && selected == index,
                            false,
                            timestamps,
                        )
                    })
                })
                .collect()
        })
        .track_scroll(&log.scroll)
        .flex_1()
        .min_h_0();
        output.child(
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(list)
                .vertical_scrollbar(&log.scroll),
        )
    };
    if log.save_failed {
        output = output.child(
            div()
                .p_2()
                .text_color(color(palette, ColorToken::StatusError))
                .child(message(
                    screen.locale,
                    gui::GUI_DESKTOP_COULDN_T_SAVE_THE_BUFFERED_LOGS,
                )),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .min_h_0()
        .flex_1()
        .child(toolbar)
        .child(source_bar)
        .child(output)
        .child(
            div()
                .text_xs()
                .text_color(color(palette, ColorToken::TextMuted))
                .child(match screen.locale {
                    marsa_ui::Locale::EnglishUs => format!("{} lines in buffer", log.rows.len()),
                    marsa_ui::Locale::Arabic => {
                        format!("{} سطر في الذاكرة المؤقتة", log.rows.len())
                    }
                }),
        )
        .into_any_element()
}

fn source_label(source: Option<LogSource>) -> marsa_ui::TextKey {
    match source {
        None => gui::GUI_DESKTOP_CONTAINERS_LOG_ALL_OUTPUT,
        Some(LogSource::Stdout) => gui::GUI_DESKTOP_CONTAINERS_LOG_STANDARD_OUTPUT,
        Some(LogSource::Stderr) => gui::GUI_DESKTOP_CONTAINERS_LOG_STANDARD_ERROR,
        Some(LogSource::Terminal) => gui::GUI_DESKTOP_CONTAINERS_LOG_TERMINAL_OUTPUT,
    }
}

fn move_match(view: &mut ContainerView, forward: bool, cx: &mut Context<ContainerView>) {
    let count = view.logs.matching.len();
    if count > 0 {
        view.logs.match_cursor = if forward {
            (view.logs.match_cursor + 1) % count
        } else {
            (view.logs.match_cursor + count - 1) % count
        };
        view.logs
            .scroll
            .scroll_to_item(view.logs.match_cursor, ScrollStrategy::Center);
        cx.notify();
    }
}

fn line_element(
    line: &Line,
    palette: marsa_ui::Palette,
    selected: bool,
    wrap: bool,
    timestamp_format: Option<(Locale, TimeFormat, bool)>,
) -> Div {
    let source = match line.content.source {
        LogSource::Stdout => "stdout",
        LogSource::Stderr => "stderr",
        LogSource::Terminal => "tty",
    };
    let foreground = if line.content.source == LogSource::Stderr {
        ColorToken::StatusWarning
    } else {
        ColorToken::TextPrimary
    };
    let timestamp = timestamp_format
        .zip(line.content.timestamp)
        .map(|((locale, format, local), timestamp)| log_time(locale, format, local, timestamp));
    // The ANSI renderer isolates the full prefix, including the timestamp.
    let prefix = line_prefix(&line.container, source, timestamp.as_deref());
    let element = div()
        .w_full()
        .px_3()
        .font(Family::Mono.font())
        .text_size(ui_size(12.))
        .line_height(ui_size(20.))
        .text_color(color(palette, foreground))
        .child(ansi::styled(&prefix, &line.content.text, palette));
    let element = if selected {
        element.bg(color(palette, ColorToken::AccentSubtle))
    } else {
        element
    };
    if wrap {
        element.py_1()
    } else {
        element
            .h(ui_size(23.))
            .overflow_hidden()
            .whitespace_nowrap()
    }
}

fn line_prefix(container: &str, source: &str, timestamp: Option<&str>) -> String {
    match timestamp {
        Some(timestamp) => format!("{timestamp}    {container}    {source:<6}    "),
        None => format!("{container}    {source:<6}    "),
    }
}

fn export(logs: &LogsState) -> String {
    logs.paused
        .as_ref()
        .unwrap_or(&logs.rows)
        .iter()
        .map(|line| {
            let text = ansi::plain(&line.content.text);
            match logs.timestamp(&line.content) {
                Some(timestamp) => format!("{timestamp} {}: {text}", line.container),
                None => format!("{}: {text}", line.container),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn save(view: &mut ContainerView, cx: &mut Context<ContainerView>) {
    let contents = export(&view.logs);
    let choice = cx.prompt_for_new_path(Path::new("."), Some("container.log"));
    let executor = cx.background_executor().clone();
    cx.spawn(async move |view, cx| {
        let Ok(Ok(Some(path))) = choice.await else {
            return;
        };
        let saved = executor
            .spawn(async move {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .and_then(|mut file| file.write_all(contents.as_bytes()))
            })
            .await;
        let _ = view.update(cx, |view, cx| {
            view.logs.save_failed = saved.is_err();
            cx.notify();
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use marsa_ui::preferences::LogBufferSize;

    use super::*;

    #[test]
    fn container_log_prefix_inserts_no_direction_controls_before_ansi_rendering() {
        for timestamp in ["2026-11-25T21:29:28Z", "2026-11-25T16:29:28-05:00"] {
            let prefix = line_prefix("example", "stdout", Some(timestamp));
            assert_eq!(
                isolate_technical_text(&prefix).as_ref(),
                format!("\u{2066}{timestamp}    example    stdout    \u{2069}")
            );
        }
        assert_eq!(
            line_prefix("example", "stderr", None),
            "example    stderr    "
        );
    }

    #[test]
    fn container_buffer_drops_oldest_rows_when_the_limit_changes() {
        let mut logs = LogsState::default();
        let container = Arc::from("test-container");
        logs.append(
            &container,
            (0..550)
                .map(|index| LogLine {
                    source: LogSource::Stdout,
                    text: index.to_string(),
                    timestamp: None,
                })
                .collect(),
        );
        assert_eq!(logs.rows.len(), 550);
        logs.settings.buffer = LogBufferSize::Compact;
        assert!(logs.trim());
        assert_eq!(logs.rows.len(), 500);
        assert_eq!(logs.rows.front().unwrap().content.text, "50");
        assert_eq!(logs.rows.back().unwrap().content.text, "549");
    }

    #[test]
    fn full_container_buffer_remains_byte_bounded() {
        let mut logs = LogsState::default();
        let container = Arc::from("test-container");
        let text = "a".repeat(64 * 1024);
        logs.append(
            &container,
            (0..40)
                .map(|_| LogLine {
                    source: LogSource::Stdout,
                    text: text.clone(),
                    timestamp: None,
                })
                .collect(),
        );
        assert_eq!(logs.rows.len(), 32);
        assert_eq!(logs.bytes, 2 * 1024 * 1024);
    }
}
