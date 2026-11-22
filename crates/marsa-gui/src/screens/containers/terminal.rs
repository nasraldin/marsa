//! Terminal chrome shared by the disabled state and future VT session view.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken,
    direction::{LogicalStyle as _, row},
    gpui::{App, Div, ParentElement as _, Styled as _, div, px},
    gpui_component::{
        Disableable as _, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
    },
};

use crate::screens::common::ScreenContext;

use super::color;

pub(super) fn render(screen: &ScreenContext<'_>, cx: &App) -> Div {
    let palette = screen.palette;
    let toolbar = row(cx)
        .gap_2()
        .items_center()
        .child(
            Button::new("start-container-shell")
                .outline()
                .small()
                .icon(IconName::Plus)
                .disabled(true)
                .label(message(screen.locale, gui::GUI_DESKTOP_NEW_SESSION)),
        )
        .child(div().flex_1())
        .child(
            Button::new("container-shell-picker")
                .outline()
                .small()
                .disabled(true)
                .label(message(screen.locale, gui::GUI_DESKTOP_SHELL)),
        )
        .child(
            Button::new("end-container-shell")
                .outline()
                .small()
                .disabled(true)
                .label(message(screen.locale, gui::GUI_DESKTOP_END_SESSION)),
        );
    let viewport = div()
        .w_full()
        .min_h(px(398.))
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .bg(color(palette, ColorToken::BgCode))
        .text_start(cx)
        .child(
            div()
                .text_color(color(palette, ColorToken::TextMuted))
                .child(message(
                    screen.locale,
                    gui::GUI_DESKTOP_THE_EMBEDDED_TERMINAL_IS_UNAVAILABLE_IN_THIS_VERSION_OF_MARSA,
                )),
        );
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(toolbar)
        .child(viewport)
}
