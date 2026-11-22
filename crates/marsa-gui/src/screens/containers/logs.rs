//! Bounded subscriptions and virtualized log output for a container or Compose project.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{collections::VecDeque, io::Write as _, path::Path, sync::Arc};

use marsa_client::docker::{ContainerId, ContainerSummary, LogLine, LogSource};
use marsa_ui::{
    ColorToken,
    direction::{isolate_technical_text, row},
    gpui::{
        AnyElement, AppContext as _, ClipboardItem, Context, Div, Entity, InteractiveElement as _,
        IntoElement, ParentElement as _, ScrollStrategy, StatefulInteractiveElement as _,
        Styled as _, Subscription, Task, UniformListScrollHandle, Window, div, px, uniform_list,
    },
    gpui_component::{
        Disableable as _, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
    },
};

use crate::{
    screens::common::{ScreenContext, label},
    services::Services,
};

use super::{ContainerView, ansi, color, name};

pub(super) const MAX_SOURCES: usize = 32;
const MAX_LINES: usize = 8_000;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct LogsState {
    rows: Arc<VecDeque<Line>>,
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
    search_input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    search_task: Option<Task<()>>,
}

#[derive(Clone)]
struct Line {
    container: Arc<str>,
    content: LogLine,
}

impl LogsState {
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
            self.bytes = self.bytes.saturating_add(line.text.len());
            rows.push_back(Line {
                container: Arc::clone(container),
                content: line,
            });
        }
        while rows.len() > MAX_LINES || self.bytes > MAX_BYTES {
            if let Some(line) = rows.pop_front() {
                self.bytes = self.bytes.saturating_sub(line.content.text.len());
            } else {
                break;
            }
        }
    }
}

pub(super) fn follow(
    services: &Services,
    id: ContainerId,
    container: Arc<str>,
    cx: &Context<ContainerView>,
) -> Task<()> {
    let mut stream = services.logs(id);
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
    let mut toolbar = row(cx).gap_2().items_center();
    if let Some(input) = log.search_input.as_ref() {
        toolbar = toolbar.child(
            div()
                .w(px(232.))
                .child(Input::new(input).small().cleanable(true)),
        );
    }
    let matches = log.matching.len();
    let filtering = !log.query.is_empty() || log.source.is_some();
    if filtering {
        toolbar = toolbar
            .child(
                Button::new("previous-log-match")
                    .outline()
                    .small()
                    .icon(IconName::ChevronLeft)
                    .accessibility_label(message(screen.locale, gui::GUI_DESKTOP_PREVIOUS_MATCH))
                    .disabled(matches == 0)
                    .on_click(cx.listener(|view, _, _, cx| move_match(view, false, cx))),
            )
            .child(
                Button::new("next-log-match")
                    .outline()
                    .small()
                    .icon(IconName::ChevronRight)
                    .accessibility_label(message(screen.locale, gui::GUI_DESKTOP_NEXT_MATCH))
                    .disabled(matches == 0)
                    .on_click(cx.listener(|view, _, _, cx| move_match(view, true, cx))),
            )
            .child(div().text_size(px(11.)).child(if matches == 0 {
                "0 / 0".to_owned()
            } else {
                format!("{} / {matches}", log.match_cursor + 1)
            }));
    }
    let (source_en, source_ar) = match log.source {
        None => ("All output", "كل المخرجات"),
        Some(LogSource::Stdout) => ("Standard output", "المخرجات القياسية"),
        Some(LogSource::Stderr) => ("Standard error", "الأخطاء القياسية"),
        Some(LogSource::Terminal) => ("Terminal output", "مخرجات الطرفية"),
    };
    toolbar = toolbar
        .child(
            Button::new("log-source-filter")
                .outline()
                .small()
                .label(label(screen, source_en, source_ar))
                .icon(IconName::ChevronDown)
                .on_click(cx.listener(|view, _, _, cx| {
                    view.logs.source = match view.logs.source {
                        None => Some(LogSource::Stdout),
                        Some(LogSource::Stdout) => Some(LogSource::Stderr),
                        Some(LogSource::Stderr) => Some(LogSource::Terminal),
                        Some(LogSource::Terminal) => None,
                    };
                    view.logs.refilter(cx);
                })),
        )
        .child(div().flex_1())
        .child(
            Button::new("log-pause")
                .outline()
                .small()
                .icon(if log.paused.is_some() {
                    IconName::Play
                } else {
                    IconName::Pause
                })
                .label(if log.paused.is_some() {
                    message(screen.locale, gui::GUI_DESKTOP_FOLLOW)
                } else {
                    message(screen.locale, gui::GUI_DESKTOP_PAUSE)
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
        .gap_2()
        .child(
            div()
                .font_family("IBM Plex Mono")
                .text_size(px(12.))
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
        source_bar = source_bar.child(div().text_size(px(11.)).child(message(
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
                        .icon(IconName::FileText)
                        .accessibility_label(message(
                            screen.locale,
                            gui::GUI_DESKTOP_SAVE_BUFFERED_LOGS,
                        ))
                        .disabled(snapshot.is_empty())
                        .on_click(cx.listener(|view, _, _, cx| save(view, cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("load-older-logs")
                        .link()
                        .small()
                        .disabled(true)
                        .label(message(screen.locale, gui::GUI_DESKTOP_LOAD_EARLIER_LOGS))
                        .tooltip(message(
                            screen.locale,
                            gui::GUI_DESKTOP_EARLIER_LOGS_AREN_T_AVAILABLE_FROM_THIS_CONNECTION,
                        )),
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
                .overflow_y_scroll()
                .children((start..row_count).filter_map(|index| {
                    let source = if filtering {
                        indexes.get(index).copied()?
                    } else {
                        index
                    };
                    snapshot.get(source).map(|line| {
                        line_element(line, palette, filtering && selected == index, true)
                    })
                })),
        )
    } else {
        output.child(
            uniform_list("container-log-buffer", row_count, move |range, _, _| {
                range
                    .filter_map(|index| {
                        let source = if filtering {
                            indexes.get(index).copied()?
                        } else {
                            index
                        };
                        snapshot.get(source).map(|line| {
                            line_element(line, palette, filtering && selected == index, false)
                        })
                    })
                    .collect()
            })
            .track_scroll(&log.scroll)
            .flex_1()
            .min_h_0(),
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
        .into_any_element()
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

fn line_element(line: &Line, palette: marsa_ui::Palette, selected: bool, wrap: bool) -> Div {
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
    let prefix = format!("{}    {source:<6}    ", line.container);
    let element = div()
        .w_full()
        .px_3()
        .font_family("IBM Plex Mono")
        .text_size(px(12.))
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
        element.h(px(23.)).overflow_hidden().whitespace_nowrap()
    }
}

fn export(logs: &LogsState) -> String {
    logs.paused
        .as_ref()
        .unwrap_or(&logs.rows)
        .iter()
        .map(|line| format!("{}: {}", line.container, ansi::plain(&line.content.text)))
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
