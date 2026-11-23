//! Translate lifecycle indicators independently from daemon diagnostic details.

use marsa_client::api::EngineState;
use marsa_ui::tokens::typography::ui_size;
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{Div, ParentElement, Styled, div, px},
};

use crate::screens::common::{color, copy, row};

pub(crate) fn text(locale: Locale, state: Option<EngineState>) -> &'static str {
    let (en, ar) = match state {
        Some(EngineState::Ready | EngineState::Idle) => ("Engine running", "المحرّك قيد التشغيل"),
        Some(EngineState::Stopped) => ("Engine stopped", "المحرّك متوقف"),
        Some(EngineState::Starting | EngineState::Booting) => {
            ("Engine starting", "جارٍ تشغيل المحرّك")
        }
        Some(EngineState::Paused) => ("Engine paused", "المحرّك متوقف مؤقتًا"),
        Some(EngineState::Stopping) => ("Engine stopping", "جارٍ إيقاف المحرّك"),
        Some(EngineState::Failed) => ("Engine failed", "تعطّل المحرّك"),
        _ => ("Daemon unavailable", "الخدمة غير متاحة"),
    };
    copy(locale, en, ar)
}

pub(crate) fn compact_badge(locale: Locale, palette: Palette, state: Option<EngineState>) -> Div {
    let text = match state {
        Some(EngineState::Ready | EngineState::Idle) => {
            marsa_ui::gui_keys::GUI_DESKTOP_RUNNING.text(locale)
        }
        Some(EngineState::Stopped) => marsa_ui::gui_keys::GUI_DESKTOP_STOPPED.text(locale),
        Some(EngineState::Paused) => marsa_ui::gui_keys::GUI_DESKTOP_PAUSED.text(locale),
        _ => text(locale, state),
    };
    badge_with_text(locale, palette, state, text)
}

pub(super) fn inline_badge(
    locale: Locale,
    palette: Palette,
    state: Option<EngineState>,
    label: &'static str,
) -> Div {
    let foreground = color(palette, foreground(state));
    row(locale)
        .gap_2()
        .text_size(ui_size(12.))
        .text_color(foreground)
        .child(
            div()
                .size(px(7.))
                .flex_shrink_0()
                .rounded_full()
                .bg(foreground),
        )
        .child(div().min_w_0().truncate().child(label))
}

pub(super) fn foreground(state: Option<EngineState>) -> ColorToken {
    themes(state).0
}

fn badge_with_text(
    locale: Locale,
    palette: Palette,
    state: Option<EngineState>,
    label: &'static str,
) -> Div {
    let (foreground, background) = themes(state);
    row(locale)
        .gap_2()
        .px_2()
        .py_1()
        .rounded_full()
        .text_xs()
        .bg(color(palette, background))
        .text_color(color(palette, foreground))
        .child(
            div()
                .size(px(7.))
                .rounded_full()
                .bg(color(palette, foreground)),
        )
        .child(label)
}

fn themes(state: Option<EngineState>) -> (ColorToken, ColorToken) {
    match state {
        Some(EngineState::Ready | EngineState::Idle) => {
            (ColorToken::StatusRunning, ColorToken::StatusRunningBg)
        }
        Some(EngineState::Starting | EngineState::Booting) => {
            (ColorToken::StatusStarting, ColorToken::StatusStartingBg)
        }
        Some(EngineState::Paused | EngineState::Stopping) => {
            (ColorToken::StatusPaused, ColorToken::StatusPausedBg)
        }
        Some(EngineState::Failed) => (ColorToken::StatusError, ColorToken::StatusErrorBg),
        _ => (ColorToken::StatusStopped, ColorToken::StatusStoppedBg),
    }
}
