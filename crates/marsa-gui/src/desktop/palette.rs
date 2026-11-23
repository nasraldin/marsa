//! The keyboard-driven search dialog from the Figma workspace shell.

mod results;

use marsa_ui::gpui_component::scroll::ScrollableElement as _;

use marsa_ui::{
    ColorToken, FocusTrapElement as _,
    gpui::{
        Context, Div, Entity, InteractiveElement, KeyDownEvent, MouseButton, ParentElement,
        StatefulInteractiveElement, Styled, Subscription, Window, div, point, px, svg,
    },
    gpui_component::{
        IconName,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
    },
    gui_keys as gui,
};

use crate::screens::common::{align, color, row, technical};

use super::Desktop;

pub(super) fn observe_query(
    query: &Entity<InputState>,
    window: &Window,
    cx: &mut Context<Desktop>,
) -> Subscription {
    cx.subscribe_in(query, window, |this, input, event, window, cx| {
        if !this.palette_open {
            return;
        }
        match event {
            InputEvent::Change => {
                this.palette_index = 0;
                this.palette_scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let entries = results::search(this, input.read(cx).value().as_ref(), cx);
                let index = this.palette_index.min(entries.len().saturating_sub(1));
                if let Some(entry) = entries.into_iter().nth(index) {
                    entry.destination.run(this, window, cx);
                }
            }
            _ => {}
        }
    })
}

impl Desktop {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            return;
        }
        self.palette_previous_focus = window.focused(cx).map(|focus| focus.downgrade());
        self.palette_open = true;
        self.palette_index = 0;
        self.palette_scroll.set_offset(point(px(0.), px(0.)));
        let locale = self.locale;
        self.query.update(cx, |input, cx| {
            input.set_placeholder(gui::GUI_PALETTE_PLACEHOLDER.text(locale), window, cx);
            input.set_value("", window, cx);
        });
        let query = self.query.clone();
        window.on_next_frame(move |window, cx| {
            query.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_open = false;
        let focus = self
            .palette_previous_focus
            .take()
            .and_then(|focus| focus.upgrade());
        focus.as_ref().unwrap_or(&self.focus).focus(window, cx);
        cx.notify();
    }

    fn step_palette(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let forward = match event.keystroke.key.as_str() {
            "down" => true,
            "up" => false,
            _ => return,
        };
        if event.keystroke.modifiers.alt
            || event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.shift
        {
            return;
        }
        let count = results::search(self, self.query.read(cx).value().as_ref(), cx).len();
        if count > 0 {
            let index = self.palette_index.min(count - 1);
            self.palette_index = if forward {
                (index + 1) % count
            } else {
                index.checked_sub(1).unwrap_or(count - 1)
            };
            self.palette_scroll.scroll_to_item(self.palette_index);
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn command_palette(&self, cx: &mut Context<Self>) -> Div {
        let palette = self.palette;
        let locale = self.locale;
        let entries = results::search(self, self.query.read(cx).value().as_ref(), cx);
        let border = color(palette, ColorToken::BorderSubtle);
        let muted = color(palette, ColorToken::TextMuted);
        let is_empty = entries.is_empty();
        let search = Input::new(&self.query)
            .aria_label(gui::GUI_PALETTE_PLACEHOLDER.text(locale))
            .prefix(IconName::Search)
            .w_full()
            .h(px(40.))
            .bg(color(palette, ColorToken::BgCode));
        let panel = div()
            .id("command-palette")
            .w(px(624.))
            .max_w_full()
            .max_h_full()
            .p_4()
            .gap_3()
            .rounded(px(16.))
            .border_1()
            .border_color(border)
            .bg(color(palette, ColorToken::BgElevated))
            .shadow(vec![marsa_ui::tokens::DIALOG_ELEVATION.shadow()])
            .flex()
            .flex_col()
            .capture_key_down(cx.listener(Self::step_palette))
            .child(div().flex_shrink_0().child(search))
            .child(
                align(div().flex_shrink_0().text_xs().text_color(muted), locale)
                    .child(gui::GUI_PALETTE_ACTIONS.text(locale)),
            )
            .child(
                div()
                    .id("palette-results")
                    .track_scroll(&self.palette_scroll)
                    .overflow_y_scrollbar()
                    .max_h(px(320.))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(is_empty.then(|| {
                        align(div().p_2().text_sm().text_color(muted), locale)
                            .child(gui::GUI_DESKTOP_NO_MATCHING_COMMANDS.text(locale))
                    }))
                    .children(entries.into_iter().enumerate().map(|(index, entry)| {
                        let selected = self.palette_index == index;
                        let foreground = if selected {
                            color(palette, ColorToken::AccentDefault)
                        } else {
                            color(palette, ColorToken::TextPrimary)
                        };
                        let title = if entry.detail.is_some() {
                            technical(&entry.title)
                        } else {
                            entry.title.into()
                        };
                        let button = Button::new(("palette-result", index))
                            .ghost()
                            .h(px(36.))
                            .rounded(px(6.))
                            .w_full()
                            .child(
                                row(locale)
                                    .w_full()
                                    .gap_3()
                                    .text_sm()
                                    .text_color(foreground)
                                    .child(
                                        svg()
                                            .path(entry.icon)
                                            .size(px(20.))
                                            .flex_shrink_0()
                                            .text_color(muted),
                                    )
                                    .child(
                                        align(div().flex_1().min_w_0().truncate(), locale)
                                            .child(title),
                                    )
                                    .children(entry.shortcut.or(entry.detail).map(|text| {
                                        div()
                                            .w(px(70.))
                                            .text_xs()
                                            .text_color(muted)
                                            .child(technical(text))
                                    })),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                entry.destination.clone().run(this, window, cx)
                            }));
                        if selected {
                            button.bg(color(palette, ColorToken::AccentSubtle))
                        } else {
                            button
                        }
                    })),
            )
            .child(
                row(locale)
                    .justify_between()
                    .gap_3()
                    .flex_shrink_0()
                    .pt_2()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        row(locale)
                            .flex_wrap()
                            .min_w_0()
                            .flex_1()
                            .gap_3()
                            .text_xs()
                            .text_color(muted)
                            .child(gui::GUI_PALETTE_NAVIGATE_HINT.text(locale))
                            .child(gui::GUI_PALETTE_OPEN_HINT.text(locale))
                            .child(gui::GUI_PALETTE_CLOSE_HINT.text(locale)),
                    )
                    .child(
                        Button::new("close-search")
                            .ghost()
                            .size(px(36.))
                            .p_0()
                            .bg(color(palette, ColorToken::GlassControl))
                            .accessibility_label(gui::GUI_DESKTOP_CLOSE.text(locale))
                            .child(IconName::Close)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                            ),
                    ),
            )
            .focus_trap("command-palette-focus", &self.palette_focus);

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .p_4()
            .child(
                div()
                    .id("palette-scrim")
                    .absolute()
                    .inset_0()
                    .bg(color(palette, ColorToken::OverlayScrim).opacity(0.45))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                    ),
            )
            .child(panel)
    }
}
