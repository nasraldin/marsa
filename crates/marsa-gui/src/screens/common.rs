//! Shared screen layout and paired English and Arabic copy.

use std::sync::Arc;

use marsa_client::docker::LogTimestamp;
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{Div, Hsla, SharedString, Styled, div, px},
    locale_format::{self, SizeFormat, day_periods},
    preferences::{DisplaySettings, TimeFormat},
    tokens::typography::{Role, TypeStyle},
};

use crate::{services::Services, store::AppState};

pub(crate) fn message(locale: Locale, key: marsa_ui::TextKey) -> SharedString {
    key.text(locale).into()
}

pub(crate) struct ScreenContext<'a> {
    pub(crate) locale: Locale,
    pub(crate) palette: Palette,
    pub(crate) display: DisplaySettings,
    pub(crate) state: &'a AppState,
    pub(crate) services: Option<&'a Arc<Services>>,
}

impl ScreenContext<'_> {
    pub(crate) fn typography(&self, role: Role) -> TypeStyle {
        role.with_ui_font(self.locale == Locale::Arabic, self.display.ui_font)
    }
}

/// Format a Docker timestamp using the display's hour cycle and log time zone.
pub(crate) fn log_time(
    locale: Locale,
    format: TimeFormat,
    local: bool,
    timestamp: LogTimestamp,
) -> String {
    match format {
        TimeFormat::TwentyFourHour => timestamp.format(local),
        TimeFormat::TwelveHour => {
            let (am, pm) = day_periods(locale);
            timestamp.format_12_hour(local, am, pm)
        }
    }
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

pub(crate) use marsa_ui::components::{
    dialogs::{dialog_actions, dialog_frame, dialog_heading},
    inputs::{resource_input, update_input_placeholder},
};

pub(crate) fn disk_bytes(locale: Locale, value: u64) -> String {
    locale_format::bytes(locale, SizeFormat::Binary, value)
}
