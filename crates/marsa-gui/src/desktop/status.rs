//! Translate lifecycle indicators independently from daemon diagnostic details.

use marsa_client::api::EngineState;
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

pub(crate) fn badge(locale: Locale, palette: Palette, state: Option<EngineState>) -> Div {
    let (foreground, background) = match state {
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
    };
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
        .child(text(locale, state))
}
