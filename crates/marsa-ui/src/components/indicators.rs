//! Compact status indicators used by resource lists.

use gpui::{Div, ParentElement as _, SharedString, Styled as _, div, px};

use crate::{
    ColorToken, Locale, Palette,
    direction::row_for,
    tokens::{
        scale::{Radius, Space},
        typography::Family,
    },
};

use super::color;

/// Show a localized status with a semantic dot and readable text.
pub fn status_pill(
    locale: Locale,
    palette: Palette,
    label: SharedString,
    foreground: ColorToken,
    background: ColorToken,
) -> Div {
    row_for(locale)
        .gap(px(Space::S8.pixels()))
        .items_center()
        .rounded(px(Radius::Full.pixels()))
        .px(px(Space::S8.pixels()))
        .py(px(Space::S4.pixels()))
        .bg(color(palette, background))
        .child(
            div()
                .size(px(Space::S8.pixels()))
                .rounded_full()
                .bg(color(palette, foreground)),
        )
        .child(
            div()
                .text_sm()
                .text_color(color(palette, foreground))
                .child(label),
        )
}

/// Compact badge without the status dot (tag count / tag name).
pub fn tag_badge(
    locale: Locale,
    palette: Palette,
    label: SharedString,
    foreground: ColorToken,
    background: ColorToken,
) -> Div {
    row_for(locale)
        .items_center()
        .min_w_0()
        .max_w_full()
        .rounded(px(Radius::Full.pixels()))
        .px(px(Space::S4.pixels()))
        .py(px(Space::S4.pixels() / 2.))
        .bg(color(palette, background))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .font_family(Family::Mono.name())
                .text_color(color(palette, foreground))
                .child(label),
        )
}
