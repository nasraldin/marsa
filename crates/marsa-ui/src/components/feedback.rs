//! Readable feedback and technical content shared by desktop views.

use gpui::{
    ElementId, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, accesskit::Role, div, px,
};

use crate::{
    ColorToken, Locale, Palette,
    direction::{align_to_start, isolate_technical_text},
    tokens::{
        scale::{Radius, Space},
        typography::Family,
    },
};

use super::color;

/// Explain why a region currently has no items.
pub fn empty_state(
    id: impl Into<ElementId>,
    locale: Locale,
    palette: Palette,
    message: impl Into<SharedString>,
) -> impl IntoElement {
    feedback(id, locale, palette, message.into(), false)
}

/// Show a localized error where its operation or data would have appeared.
pub fn error_state(
    id: impl Into<ElementId>,
    locale: Locale,
    palette: Palette,
    message: impl Into<SharedString>,
) -> impl IntoElement {
    feedback(id, locale, palette, message.into(), true)
}

fn feedback(
    id: impl Into<ElementId>,
    locale: Locale,
    palette: Palette,
    message: SharedString,
    error: bool,
) -> impl IntoElement {
    let (role, foreground, background) = if error {
        (
            Role::Alert,
            ColorToken::StatusError,
            ColorToken::StatusErrorBg,
        )
    } else {
        (Role::Status, ColorToken::TextMuted, ColorToken::BgSubtle)
    };
    align_to_start(div(), locale)
        .id(id)
        .role(role)
        .p(px(Space::S12.pixels()))
        .rounded(px(Radius::R8.pixels()))
        .bg(color(palette, background))
        .text_color(color(palette, foreground))
        .child(message)
}

/// Present a path, command, or other literal in an isolated monospace run.
pub fn monospace_block(locale: Locale, palette: Palette, value: &str) -> impl IntoElement {
    align_to_start(div(), locale)
        .p(px(Space::S12.pixels()))
        .rounded(px(Radius::R8.pixels()))
        .bg(color(palette, ColorToken::BgCode))
        .text_color(color(palette, ColorToken::TextPrimary))
        .font_family(Family::Mono.name())
        .child(isolate_technical_text(value))
}
