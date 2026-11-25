//! Interactive Docker exec sessions with native input and a bounded VT screen.

mod ime;

use std::num::NonZeroU16;

use marsa_client::docker::{ContainerId, ContainerSummary, MAX_TERMINAL_INPUT};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Palette,
    direction::{LogicalStyle as _, isolate_technical_text, row},
    gpui::{
        AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, Div, ElementId,
        ElementInputHandler, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement,
        KeyDownEvent, ParentElement as _, Pixels, Render, ScrollEvent,
        StatefulInteractiveElement as _, Styled as _, Subscription, Task, TitlebarOptions,
        WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, canvas, div, point, px,
        size,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
    },
    tokens::{
        Rgb,
        typography::{Family, Role, rem_size, ui_size},
    },
    vt100::{Color, Parser, Screen},
};

use crate::{
    screens::common::{ScreenContext, message},
    services::{TerminalInput, TerminalOutput},
};

use super::{ContainerView, color, name};

const MAX_SESSIONS: usize = 8;
const SCROLLBACK: usize = 2_000;
const FONT_SIZE: Pixels = px(12.);
const CELL_HEIGHT: Pixels = px(18.);

fn cell_width(window: &Window, family: Family) -> Pixels {
    let font = window.text_system().resolve_font(&family.font());
    window
        .text_system()
        .advance(font, FONT_SIZE, 'M')
        .map(|size| size.width)
        .unwrap_or(px(7.2))
        .max(px(1.))
}

#[derive(Default)]
pub(super) struct State {
    sessions: Vec<Session>,
    selected: Option<u64>,
    next_id: u64,
    focus: Option<FocusHandle>,
    compose: ime::Composition,
    detached: Option<WindowHandle<Detached>>,
}

struct Session {
    id: u64,
    parser: Parser,
    input: TerminalInput,
    opened: bool,
    closed: bool,
    failed: bool,
    input_failed: bool,
    _task: Task<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.input.close();
    }
}

impl State {
    pub(super) fn prepare(&mut self, cx: &mut Context<ContainerView>) {
        if self.focus.is_none() {
            self.focus = Some(cx.focus_handle());
        }
    }

    pub(super) fn close_detached(&mut self, cx: &mut App) {
        if let Some(handle) = self.detached.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn current(&self) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|session| Some(session.id) == self.selected)
    }

    fn current_mut(&mut self) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .find(|session| Some(session.id) == self.selected)
    }

    fn close(&mut self, id: u64) {
        self.sessions.retain(|session| session.id != id);
        self.compose = ime::Composition::default();
        if self.selected == Some(id) {
            self.selected = self.sessions.last().map(|session| session.id);
        }
    }
}

fn start(
    view: &mut ContainerView,
    container: ContainerId,
    window: &mut Window,
    cx: &mut Context<ContainerView>,
) {
    let Some(services) = view.services.as_ref() else {
        return;
    };
    let Some(target) = view
        .focused
        .as_ref()
        .filter(|target| target.id() == &container)
    else {
        return;
    };
    if !view
        .target_state(target)
        .and_then(|state| state.container(&container))
        .is_some_and(|entry| entry.state == "running")
    {
        return;
    }
    let endpoint = target.endpoint().cloned();
    let terminal = &mut view.terminal;
    if terminal.sessions.len() >= MAX_SESSIONS {
        return;
    }
    let id = terminal.next_id;
    terminal.next_id = terminal.next_id.saturating_add(1);
    let parser = Parser::new(Parser::DEFAULT_ROWS, Parser::DEFAULT_COLS, SCROLLBACK);
    let mut wire = services.terminal(
        container,
        endpoint,
        Parser::DEFAULT_COLS.get(),
        Parser::DEFAULT_ROWS.get(),
    );
    let input = wire.input();
    let task = cx.spawn(async move |view, cx| {
        while let Some(output) = wire.next().await {
            let result = view.update(cx, |view, cx| {
                let Some(session) = view
                    .terminal
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == id)
                else {
                    return false;
                };
                match output {
                    Ok(TerminalOutput::Opened) => session.opened = true,
                    Ok(TerminalOutput::Bytes(bytes)) => session.parser.process(&bytes),
                    Err(_) => session.failed = true,
                }
                cx.notify();
                true
            });
            if !matches!(result, Ok(true)) {
                return;
            }
        }
        let _ = view.update(cx, |view, cx| {
            if let Some(session) = view
                .terminal
                .sessions
                .iter_mut()
                .find(|session| session.id == id)
            {
                session.closed = true;
                cx.notify();
            }
        });
    });
    terminal.sessions.push(Session {
        id,
        parser,
        input,
        opened: false,
        closed: false,
        failed: false,
        input_failed: false,
        _task: task,
    });
    terminal.selected = Some(id);
    if let Some(focus) = terminal.focus.as_ref() {
        focus.focus(window, cx);
    }
    cx.notify();
}

pub(super) fn render(
    view: &ContainerView,
    entry: &ContainerSummary,
    screen: &ScreenContext<'_>,
    window: &Window,
    cx: &mut Context<ContainerView>,
) -> AnyElement {
    let panel = &view.terminal;
    let palette = screen.palette;
    let id = entry.id.clone();
    let mut buttons = row(cx).gap_2().items_center().flex_wrap();
    for session in &panel.sessions {
        let session_id = session.id;
        let title = isolate_technical_text(&format!("sh · {}", session_id + 1));
        let button = Button::new((
            ElementId::from("container-shell-tab"),
            session_id.to_string(),
        ))
        .outline()
        .small()
        .label(title)
        .toggled(panel.selected == Some(session_id))
        .disabled(panel.detached.is_some());
        buttons = buttons.child(
            if panel.selected == Some(session_id) {
                button
                    .ghost()
                    .bg(color(palette, ColorToken::AccentSubtle))
                    .text_color(color(palette, ColorToken::AccentDefault))
            } else {
                button.ghost()
            }
            .on_click(cx.listener(move |view, _, window, cx| {
                view.terminal.selected = Some(session_id);
                view.terminal.compose = ime::Composition::default();
                if let Some(focus) = view.terminal.focus.as_ref() {
                    focus.focus(window, cx);
                }
                cx.notify();
            })),
        );
    }
    buttons = buttons
        .child(
            Button::new("new-terminal-shell")
                .outline()
                .small()
                .icon(IconName::Plus)
                .label(message(screen.locale, gui::GUI_DESKTOP_NEW_SESSION))
                .disabled(
                    view.services.is_none()
                        || entry.state != "running"
                        || panel.sessions.len() >= MAX_SESSIONS
                        || panel.detached.is_some(),
                )
                .on_click(
                    cx.listener(move |view, _, window, cx| start(view, id.clone(), window, cx)),
                ),
        )
        .child(
            Button::new("detach-container-terminal")
                .ghost()
                .small()
                .icon(Icon::empty().path("icons/external-link.svg"))
                .disabled(panel.current().is_none() || panel.detached.is_some())
                .tooltip(gui::GUI_DESKTOP_CONTAINERS_DETACH_TERMINAL.text(screen.locale))
                .accessibility_label(
                    gui::GUI_DESKTOP_CONTAINERS_DETACH_TERMINAL.text(screen.locale),
                )
                .on_click(cx.listener(|view, _, _, cx| detach(view, cx))),
        )
        .child(div().flex_1())
        .child(
            Button::new("copy-terminal-screen")
                .outline()
                .small()
                .icon(IconName::Copy)
                .accessibility_label(message(screen.locale, gui::GUI_CONTAINER_TERMINAL_COPY))
                .disabled(panel.current().is_none())
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Some(session) = view.terminal.current() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            session.parser.screen().contents(),
                        ));
                    }
                })),
        )
        .child(
            Button::new("end-terminal-shell")
                .outline()
                .small()
                .label(message(screen.locale, gui::GUI_DESKTOP_END_SESSION))
                .disabled(panel.current().is_none() || panel.detached.is_some())
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Some(id) = view.terminal.selected {
                        view.terminal.close(id);
                    }
                    cx.notify();
                })),
        );
    let terminal = if panel.detached.is_some() {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .rounded_lg()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .bg(color(palette, ColorToken::BgCode))
            .text_color(color(palette, ColorToken::TextMuted))
            .child(gui::GUI_DESKTOP_CONTAINERS_TERMINAL_DETACHED.text(screen.locale))
            .into_any_element()
    } else {
        viewport(view, entry, screen, window, cx).into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_3()
        .child(buttons)
        .child(terminal)
        .into_any_element()
}

struct Detached {
    owner: WeakEntity<ContainerView>,
    _observation: Subscription,
    ready: bool,
}

fn detach(view: &mut ContainerView, cx: &mut Context<ContainerView>) {
    if view.terminal.current().is_none() || view.terminal.detached.is_some() {
        return;
    }
    let owner = cx.entity();
    let weak = owner.downgrade();
    let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(520.), px(320.))),
        titlebar: Some(TitlebarOptions {
            title: Some(
                gui::GUI_DESKTOP_CONTAINERS_TERMINAL_TAB
                    .text(view.locale)
                    .into(),
            ),
            appears_transparent: true,
            traffic_light_position: Some(point(px(24.), px(22.))),
        }),
        ..WindowOptions::default()
    };
    let window = cx.open_window(options, |window, cx| {
        let closing = weak.clone();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = closing.update(cx, |view, cx| {
                view.terminal.detached = None;
                view.terminal.compose = ime::Composition::default();
                cx.notify();
            });
            true
        });
        cx.new(|cx: &mut Context<Detached>| Detached {
            owner: weak.clone(),
            _observation: cx.observe(&owner, |_, _, cx| cx.notify()),
            ready: false,
        })
    });
    if let Ok(window) = window {
        view.terminal.detached = Some(window);
        view.terminal.compose = ime::Composition::default();
        let focus = view.terminal.focus.clone();
        let _ = window.update(cx, |detached, window, cx| {
            detached.ready = true;
            if let Some(focus) = &focus {
                focus.focus(window, cx);
            }
            cx.notify();
        });
    }
    cx.notify();
}

impl Render for Detached {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.ready {
            return div();
        }
        self.owner
            .update(cx, |view, cx| {
                let screen = view.screen();
                let target = view.focused.as_ref()?;
                let entry = view.target_state(target)?.container(target.id())?;
                let palette = screen.palette;
                let terminal_font = Family::available_terminal(screen.display.terminal_font, cx);
                let heading = format!("{}  ·  /bin/sh", name(entry));
                window.set_rem_size(px(rem_size(screen.display.ui_font_size)));
                Some(
                    screen
                        .typography(Role::Body)
                        .apply(div())
                        .flex()
                        .flex_col()
                        .size_full()
                        .min_h_0()
                        .p_3()
                        .gap_3()
                        .bg(color(palette, ColorToken::BgCanvas))
                        .text_color(color(palette, ColorToken::TextPrimary))
                        .child(
                            row(cx)
                                .h(px(42.))
                                .items_center()
                                .gap_2()
                                .child(div().w(px(84.)))
                                .child(
                                    div()
                                        .flex_1()
                                        .font(terminal_font.font())
                                        .text_xs()
                                        .child(isolate_technical_text(&heading)),
                                )
                                .child(
                                    Button::new("return-detached-terminal")
                                        .ghost()
                                        .size(px(36.))
                                        .p_0()
                                        .bg(color(palette, ColorToken::BgSubtle))
                                        .icon(IconName::ChevronLeft)
                                        .tooltip(
                                            gui::GUI_DESKTOP_CONTAINERS_DOCK_TERMINAL
                                                .text(screen.locale),
                                        )
                                        .accessibility_label(
                                            gui::GUI_DESKTOP_CONTAINERS_DOCK_TERMINAL
                                                .text(screen.locale),
                                        )
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.terminal.detached = None;
                                            view.terminal.compose = ime::Composition::default();
                                            window.remove_window();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("fullscreen-detached-terminal")
                                        .ghost()
                                        .size(px(36.))
                                        .p_0()
                                        .bg(color(palette, ColorToken::BgSubtle))
                                        .icon(Icon::empty().path("icons/monitor.svg"))
                                        .tooltip(
                                            gui::GUI_DESKTOP_CONTAINERS_FULLSCREEN_TERMINAL
                                                .text(screen.locale),
                                        )
                                        .accessibility_label(
                                            gui::GUI_DESKTOP_CONTAINERS_FULLSCREEN_TERMINAL
                                                .text(screen.locale),
                                        )
                                        .on_click(|_, window, _| window.toggle_fullscreen()),
                                ),
                        )
                        .child(viewport(view, entry, &screen, window, cx)),
                )
            })
            .ok()
            .flatten()
            .unwrap_or_else(div)
    }
}

fn viewport(
    view: &ContainerView,
    entry: &ContainerSummary,
    screen: &ScreenContext<'_>,
    window: &Window,
    cx: &mut Context<ContainerView>,
) -> impl IntoElement {
    let panel = &view.terminal;
    let palette = screen.palette;
    let family = Family::available_terminal(screen.display.terminal_font, cx);
    let cell_width = cell_width(window, family);
    let mut terminal = div()
        .id("container-terminal-viewport")
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h(px(240.))
        .overflow_hidden()
        .rounded_lg()
        .p_4()
        .bg(color(palette, ColorToken::BgCode))
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle));
    if let Some(focus) = panel.focus.as_ref() {
        let focus = focus.clone();
        terminal = terminal
            .track_focus(&focus)
            .on_click(move |_, window, cx| focus.focus(window, cx));
    }
    terminal = terminal
        .on_key_down(cx.listener(handle_key))
        .on_scroll(cx.listener(scroll));
    if let Some(session) = panel.current() {
        let status = if session.input_failed {
            gui::GUI_CONTAINER_TERMINAL_INPUT_FAILED
        } else if session.failed {
            gui::GUI_CONTAINER_TERMINAL_CONNECTION_FAILED
        } else if session.closed {
            gui::GUI_CONTAINER_TERMINAL_ENDED
        } else if session.opened {
            gui::GUI_CONTAINER_TERMINAL_CONNECTED
        } else {
            gui::GUI_CONTAINER_TERMINAL_OPENING
        };
        terminal = terminal.child(
            div()
                .text_start(cx)
                .text_size(ui_size(12.))
                .text_color(color(palette, ColorToken::TextMuted))
                .child(message(screen.locale, status)),
        );
        let input_entity = cx.entity();
        let input_focus = panel
            .focus
            .clone()
            .filter(|_| session.opened && !session.closed);
        let entity = cx.entity().downgrade();
        let session_id = session.id;
        let dimensions = session.parser.screen().size();
        let observer = canvas(
            move |bounds, window, _| {
                let rows = (f32::from(bounds.size.height) / f32::from(CELL_HEIGHT))
                    .floor()
                    .clamp(8., 64.) as u16;
                let cols = (f32::from(bounds.size.width) / f32::from(cell_width))
                    .floor()
                    .clamp(20., 160.) as u16;
                let new = (NonZeroU16::new(rows), NonZeroU16::new(cols));
                if let (Some(rows), Some(cols)) = new
                    && (rows, cols) != dimensions
                {
                    let entity = entity.clone();
                    window.on_next_frame(move |_, cx| {
                        let _ = entity.update(cx, |view, cx| {
                            if let Some(session) = view
                                .terminal
                                .current_mut()
                                .filter(|session| session.id == session_id)
                                && session.input.resize(cols.get(), rows.get())
                            {
                                session.parser.set_size(rows, cols);
                                cx.notify();
                            }
                        });
                    });
                }
            },
            move |bounds, (), window, cx| {
                if let Some(focus) = &input_focus {
                    window.handle_input(focus, ElementInputHandler::new(bounds, input_entity), cx);
                }
            },
        );
        terminal = terminal.child(
            div()
                .relative()
                .w_full()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(div().absolute().inset_0().child(observer.size_full()))
                .child(screen_lines(
                    session.parser.screen(),
                    palette,
                    family,
                    cell_width,
                    session.opened && !session.closed,
                    &panel.compose.marked,
                )),
        );
    } else {
        let key = if entry.state == "running" {
            gui::GUI_CONTAINER_TERMINAL_OPEN
        } else {
            gui::GUI_CONTAINER_TERMINAL_START_REQUIRED
        };
        terminal = terminal.child(
            div()
                .text_start(cx)
                .text_color(color(palette, ColorToken::TextMuted))
                .child(message(screen.locale, key)),
        );
    }
    terminal
}

fn screen_lines(
    screen: &Screen,
    palette: Palette,
    family: Family,
    cell_width: Pixels,
    active: bool,
    marked: &str,
) -> Div {
    let (rows, cols) = screen.size();
    let cursor = (active && !screen.hide_cursor() && screen.scrollback() == 0)
        .then(|| screen.cursor_position());
    div()
        .flex()
        .flex_col()
        .font(family.font())
        .text_size(FONT_SIZE)
        .line_height(CELL_HEIGHT)
        .children((0..rows.get()).map(|y| {
            let cells = (0..cols.get()).filter_map(|column| {
                let cell = screen.cell(y, column)?;
                if cell.is_wide_continuation() {
                    return None;
                }
                let selected = cursor == Some((y, column));
                let inverse = cell.inverse() || selected;
                let foreground = ansi_color(
                    if inverse {
                        cell.bgcolor()
                    } else {
                        cell.fgcolor()
                    },
                    palette,
                    if inverse {
                        ColorToken::BgCode
                    } else {
                        ColorToken::TextPrimary
                    },
                );
                let background = ansi_color(
                    if inverse {
                        cell.fgcolor()
                    } else {
                        cell.bgcolor()
                    },
                    palette,
                    if inverse {
                        ColorToken::AccentDefault
                    } else {
                        ColorToken::BgCode
                    },
                );
                let value = if selected && !marked.is_empty() {
                    isolate_technical_text(marked)
                } else if cell.has_contents() {
                    isolate_technical_text(cell.contents())
                } else {
                    " ".into()
                };
                let width = if cell.is_wide() {
                    cell_width * 2.0
                } else {
                    cell_width
                };
                Some(
                    div()
                        .w(width)
                        .h(CELL_HEIGHT)
                        .flex_shrink_0()
                        .overflow_hidden()
                        .text_color(foreground)
                        .bg(background)
                        .font_weight(if cell.bold() {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .child(value),
                )
            });
            div()
                .flex()
                .h(CELL_HEIGHT)
                .whitespace_nowrap()
                .children(cells)
        }))
}

fn ansi_color(color_value: Color, palette: Palette, fallback: ColorToken) -> Hsla {
    match color_value {
        Color::Default => color(palette, fallback),
        Color::Rgb(red, green, blue) => Rgb::from_channels(red, green, blue).into(),
        Color::Idx(index @ 0..=15) => {
            let token = match index % 8 {
                0 => ColorToken::TextMuted,
                1 => ColorToken::StatusError,
                2 => ColorToken::StatusRunning,
                3 => ColorToken::StatusWarning,
                4 => ColorToken::AccentDefault,
                5 => ColorToken::ChartMemory,
                6 => ColorToken::ChartNetwork,
                _ => ColorToken::TextPrimary,
            };
            color(palette, token)
        }
        Color::Idx(index @ 16..=231) => {
            let index = index - 16;
            let level = |channel| if channel == 0 { 0 } else { 55 + channel * 40 };
            Rgb::from_channels(level(index / 36), level(index / 6 % 6), level(index % 6)).into()
        }
        Color::Idx(index) => {
            let level = 8 + (index - 232) * 10;
            Rgb::from_channels(level, level, level).into()
        }
    }
}

fn scroll(
    view: &mut ContainerView,
    event: &ScrollEvent,
    _: &mut Window,
    cx: &mut Context<ContainerView>,
) {
    let Some(session) = view.terminal.current_mut() else {
        return;
    };
    let shift = f32::from(event.delta.pixel_delta(CELL_HEIGHT).y) / f32::from(CELL_HEIGHT);
    let current = session.parser.screen().scrollback();
    let next = current
        .saturating_add_signed(shift.round() as isize)
        .min(SCROLLBACK);
    session.parser.screen_mut().set_scrollback(next);
    cx.notify();
    cx.stop_propagation();
}

fn handle_key(
    view: &mut ContainerView,
    event: &KeyDownEvent,
    _: &mut Window,
    cx: &mut Context<ContainerView>,
) {
    let Some(session) = view
        .terminal
        .current_mut()
        .filter(|session| session.opened && !session.closed)
    else {
        return;
    };
    let key = &event.keystroke;
    let modifiers = key.modifiers;
    if modifiers.platform {
        match key.key.as_str() {
            "c" => cx.write_to_clipboard(ClipboardItem::new_string(
                session.parser.screen().contents(),
            )),
            "v" => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return;
                };
                // Never turn a pasted escape character into an emulated key sequence.
                let text = text.replace('\u{1b}', "");
                let text = if session.parser.screen().bracketed_paste() {
                    format!("\u{1b}[200~{text}\u{1b}[201~")
                } else {
                    text
                };
                if text.len() <= MAX_TERMINAL_INPUT {
                    session.input_failed = !session.input.write(text.into_bytes());
                } else {
                    session.input_failed = true;
                }
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
        return;
    }
    let screen = session.parser.screen();
    let cursor = if screen.application_cursor() {
        "\u{1b}O"
    } else {
        "\u{1b}["
    };
    let sequence = match key.key.as_str() {
        "up" => Some(format!("{cursor}A")),
        "down" => Some(format!("{cursor}B")),
        "right" => Some(format!("{cursor}C")),
        "left" => Some(format!("{cursor}D")),
        "home" => Some(format!("{cursor}H")),
        "end" => Some(format!("{cursor}F")),
        "f1" => Some("\u{1b}OP".into()),
        "f2" => Some("\u{1b}OQ".into()),
        "f3" => Some("\u{1b}OR".into()),
        "f4" => Some("\u{1b}OS".into()),
        "f5" => Some("\u{1b}[15~".into()),
        "f6" => Some("\u{1b}[17~".into()),
        "f7" => Some("\u{1b}[18~".into()),
        "f8" => Some("\u{1b}[19~".into()),
        "f9" => Some("\u{1b}[20~".into()),
        "f10" => Some("\u{1b}[21~".into()),
        "f11" => Some("\u{1b}[23~".into()),
        "f12" => Some("\u{1b}[24~".into()),
        "pageup" => Some("\u{1b}[5~".into()),
        "pagedown" => Some("\u{1b}[6~".into()),
        "delete" => Some("\u{1b}[3~".into()),
        "escape" => Some("\u{1b}".into()),
        "backspace" => Some("\u{7f}".into()),
        "tab" if modifiers.shift => Some("\u{1b}[Z".into()),
        "tab" => Some("\t".into()),
        "enter" | "return" => Some("\r".into()),
        _ => None,
    };
    let bytes = if let Some(sequence) = sequence {
        sequence.into_bytes()
    } else if modifiers.control {
        let [letter] = key.key.as_bytes() else { return };
        if letter.is_ascii_alphabetic() {
            vec![letter.to_ascii_uppercase() & 0x1f]
        } else {
            return;
        }
    } else {
        // Native textual input delivers ordinary characters and IME commits.
        // Returning here lets macOS pass the key to the installed input handler.
        return;
    };
    session.parser.screen_mut().set_scrollback(0);
    session.input_failed = !session.input.write(bytes);
    cx.notify();
    cx.stop_propagation();
}
