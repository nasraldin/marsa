//! Floating operation feedback that leaves the current screen in place.

use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::LogicalStyle,
    gpui::{
        Animation, AnimationExt, AnyElement, App, ElementId, InteractiveElement, IntoElement,
        ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, div, ease_out_quint,
        px, svg,
    },
    gpui_component::button::Button,
    tokens::scale::Motion,
};

use crate::screens::common::{align, color, row};

/// Match notification icons and colors to the importance of the message.
#[derive(Clone, Copy)]
pub(crate) enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

impl Tone {
    fn icon(self) -> (&'static str, ColorToken) {
        match self {
            Self::Info => ("icons/info.svg", ColorToken::AccentDefault),
            Self::Success => ("icons/circle-check.svg", ColorToken::StatusRunning),
            Self::Warning => ("icons/triangle-alert.svg", ColorToken::StatusWarning),
            Self::Error => ("icons/circle-alert.svg", ColorToken::StatusError),
        }
    }
}

/// Anchor a toast above the content at the logical bottom-end of the workspace.
pub(crate) fn render(
    id: impl Into<ElementId>,
    locale: Locale,
    palette: Palette,
    tone: Tone,
    message: impl Into<SharedString>,
    dismiss: Option<Button>,
    cx: &App,
) -> AnyElement {
    let id = id.into();
    let (icon, tint) = tone.icon();
    let content = row(locale)
        .id(id.clone())
        .role(match tone {
            Tone::Info | Tone::Success => Role::Status,
            Tone::Warning | Tone::Error => Role::Alert,
        })
        .items_start()
        .gap_3()
        .p_3()
        .rounded(px(12.))
        .bg(color(palette, ColorToken::BgElevated))
        .shadow_lg()
        .child(
            svg()
                .path(icon)
                .size(px(16.))
                .flex_shrink_0()
                .mt_0p5()
                .text_color(color(palette, tint)),
        )
        .child(
            align(div().min_w_0().flex_1(), locale)
                .text_sm()
                .text_color(color(palette, ColorToken::TextPrimary))
                .child(message.into()),
        )
        .children(dismiss);

    div()
        .absolute()
        .bottom(px(24.))
        .inset_end(px(24.), cx)
        .w(px(384.))
        .child(content)
        .with_animation(
            id,
            Animation::new(Motion::Normal.duration(false)).with_easing(ease_out_quint()),
            |toast, progress| {
                toast
                    .bottom(px(19. + progress * 5.))
                    .opacity(0.72 + progress * 0.28)
            },
        )
        .into_any_element()
}
