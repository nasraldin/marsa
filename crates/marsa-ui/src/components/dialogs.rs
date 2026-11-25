//! Native dialog framing for desktop and gallery views.

use gpui::{
    Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString, Styled as _, div,
    px, svg, transparent_black,
};
use gpui_base::DialogClose;
use gpui_component::dialog::Dialog;

use crate::{
    ColorToken, Locale, Palette,
    direction::{LogicalStyle as _, align_to_start as align, row_for as row},
    tokens::{
        scale::{Radius, Space},
        typography::ui_size,
    },
};

use super::color;

/// Give native dialogs consistent insets while letting the toolkit clamp to the window.
pub fn dialog_frame(dialog: Dialog, width: f32) -> Dialog {
    dialog
        .centered()
        .width(px(width))
        .p(px(Space::S24.pixels()))
        .gap_4()
        .close_button(false)
}

/// Lay out a wrapping title and localized dismiss control in reading order.
pub fn dialog_heading(locale: Locale, palette: Palette, title: impl Into<SharedString>) -> Div {
    row(locale)
        .w_full()
        .items_start()
        .gap_3()
        .pb_3()
        .border_b_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .child(
            align(div().flex_1().min_w_0(), locale)
                .text_size(ui_size(21.))
                .line_height(ui_size(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(palette, ColorToken::TextPrimary))
                .child(title.into()),
        )
        .child(DialogClose::new().trigger(move |button| {
            let close = crate::gui_keys::GUI_DESKTOP_CLOSE.text(locale);
            button
                .size(px(Space::S32.pixels()))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(Radius::R6.pixels()))
                .border_1()
                .border_color(transparent_black())
                .focus_visible(|style| style.border_color(color(palette, ColorToken::AccentFocus)))
                .hover(|style| style.bg(color(palette, ColorToken::GlassControl)))
                .accessibility_label(close)
                .child(
                    svg()
                        .path("icons/close.svg")
                        .size(px(Space::S16.pixels()))
                        .text_color(color(palette, ColorToken::TextMuted)),
                )
        }))
}

/// Keep the action band visible as the dialog body scrolls.
pub fn dialog_actions(locale: Locale, palette: Palette) -> Div {
    row(locale)
        .flex_wrap()
        .justify_row_end()
        .gap_2()
        .pt_3()
        .border_t_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
}
