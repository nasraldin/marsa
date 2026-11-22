//! Shared screen layout and paired English and Arabic copy.

use std::sync::Arc;

use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{Div, Hsla, SharedString, Styled, div, px},
};

use crate::{services::Services, store::AppState};

pub(crate) fn message(locale: Locale, key: marsa_ui::TextKey) -> SharedString {
    key.text(locale).into()
}

pub(crate) struct ScreenContext<'a> {
    pub(crate) locale: Locale,
    pub(crate) palette: Palette,
    pub(crate) state: &'a AppState,
    pub(crate) services: Option<&'a Arc<Services>>,
}

pub(crate) fn copy(locale: Locale, en: &'static str, ar: &'static str) -> &'static str {
    match locale {
        Locale::Arabic => ar,
        _ => en,
    }
}

pub(crate) fn label(
    screen: &ScreenContext<'_>,
    en: &'static str,
    ar: &'static str,
) -> SharedString {
    copy(screen.locale, en, ar).into()
}

pub(crate) fn color(palette: Palette, token: ColorToken) -> Hsla {
    palette.color(token).into()
}

pub(crate) fn panel(screen: &ScreenContext<'_>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded(px(12.0))
        .border_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .bg(color(screen.palette, ColorToken::BgElevated))
}

pub(crate) use marsa_ui::direction::{align_to_start as align, row_for as row};

pub(crate) fn technical(value: &str) -> SharedString {
    marsa_ui::direction::isolate_technical_text(value)
}
