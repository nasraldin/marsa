//! Shared screen layout and paired English and Arabic copy.

use std::sync::Arc;

use marsa_client::docker::LogTimestamp;
use marsa_ui::{
    ColorToken, DialogClose, Locale, Palette,
    direction::LogicalStyle as _,
    gpui::{
        App, Div, Entity, FontWeight, Hsla, InteractiveElement, ParentElement, SharedString,
        Styled, Window, div, px, svg,
    },
    gpui_component::{
        dialog::Dialog,
        input::{Input, InputState},
    },
    preferences::{DisplaySettings, TimeFormat},
    tokens::{
        scale::Space,
        typography::{Role, TypeStyle, ui_size},
    },
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

/// The locale's markers for the two halves of a 12-hour clock.
pub(crate) fn day_periods(locale: Locale) -> (&'static str, &'static str) {
    (
        marsa_ui::gui_keys::GUI_CLOCK_AM.text(locale),
        marsa_ui::gui_keys::GUI_CLOCK_PM.text(locale),
    )
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

/// Give native dialogs consistent insets while letting the toolkit clamp to the window.
pub(crate) fn dialog_frame(dialog: Dialog, width: f32) -> Dialog {
    dialog
        .centered()
        .width(px(width))
        .p(px(24.))
        .gap_4()
        .close_button(false)
}

/// Lay out a wrapping title and localized dismiss control in reading order.
pub(crate) fn dialog_heading(
    locale: Locale,
    palette: Palette,
    title: impl Into<SharedString>,
) -> Div {
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
            let close = marsa_ui::gui_keys::GUI_DESKTOP_CLOSE.text(locale);
            button
                .size(px(30.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(marsa_ui::gpui::transparent_black())
                .focus_visible(|style| style.border_color(color(palette, ColorToken::AccentFocus)))
                .hover(|style| style.bg(color(palette, ColorToken::GlassControl)))
                .accessibility_label(close)
                .child(
                    svg()
                        .path("icons/close.svg")
                        .size(px(16.))
                        .text_color(color(palette, ColorToken::TextMuted)),
                )
        }))
}

/// Keep the action band visible as the dialog body scrolls.
pub(crate) fn dialog_actions(locale: Locale, palette: Palette) -> Div {
    row(locale)
        .flex_wrap()
        .justify_row_end()
        .gap_2()
        .pt_3()
        .border_t_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
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

pub(crate) fn resource_input(state: &Entity<InputState>) -> Input {
    Styled::h(Input::new(state), px(Space::S36.pixels()))
}

pub(crate) fn update_input_placeholder(
    state: &Entity<InputState>,
    value: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).presentation().placeholder() != value {
        state.update(cx, |state, cx| state.set_placeholder(value, window, cx));
    }
}

pub(crate) fn disk_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024. && unit + 1 < UNITS.len() {
        amount /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else if amount.fract() == 0. {
        format!("{amount:.0} {}", UNITS[unit])
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}
