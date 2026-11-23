//! Empty and disconnected states for the Containers workspace.

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken, Locale, TextKey,
    direction::isolate_technical_text,
    gpui::{Context, Div, ParentElement as _, Styled as _, div, px},
    gpui_component::{
        Disableable as _, Icon, IconName,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use crate::{desktop::StartEngineFromScreen, screens::common::ScreenContext};

use super::{ContainerView, StatusFilter, color};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TableState {
    Rows,
    Loading,
    NoMatches,
    Empty,
    Stopped,
    Offline,
    Failed,
    Paused,
}

impl TableState {
    pub(super) fn from(view: &ContainerView, has_rows: bool) -> Self {
        match view.state.engine().map(|engine| engine.state()) {
            Some(EngineState::Ready | EngineState::Idle) => {
                if has_rows {
                    Self::Rows
                } else if !view.query.trim().is_empty() || view.filter != StatusFilter::All {
                    Self::NoMatches
                } else {
                    Self::Empty
                }
            }
            Some(EngineState::Starting | EngineState::Booting | EngineState::Stopping) => {
                Self::Loading
            }
            Some(EngineState::Stopped) => Self::Stopped,
            Some(EngineState::Paused) => Self::Paused,
            Some(EngineState::Failed) => Self::Failed,
            None if view.services.is_some() && !view.connection_waited => Self::Loading,
            _ => Self::Offline,
        }
    }

    pub(super) fn can_create(self) -> bool {
        matches!(self, Self::Rows | Self::Empty | Self::NoMatches)
    }
}

pub(super) fn subtitle(state: TableState, locale: Locale) -> &'static str {
    match state {
        TableState::Rows | TableState::NoMatches => {
            gui::GUI_DESKTOP_CONTAINERS_WORKSPACE_DESCRIPTION
        }
        TableState::Stopped | TableState::Paused => gui::GUI_DESKTOP_CONTAINERS_STOPPED_DESCRIPTION,
        TableState::Offline | TableState::Failed => gui::GUI_DESKTOP_CONTAINERS_OFFLINE_DESCRIPTION,
        _ => gui::GUI_DESKTOP_CONTAINERS_EMPTY_DESCRIPTION,
    }
    .text(locale)
}

pub(super) fn render(
    view: &ContainerView,
    state: TableState,
    screen: &ScreenContext<'_>,
    cx: &mut Context<ContainerView>,
) -> Div {
    let (path, title, detail): (&str, TextKey, TextKey) = match state {
        TableState::Loading => (
            "icons/loader-circle.svg",
            gui::GUI_DESKTOP_CONTAINERS_LOADING,
            gui::GUI_DESKTOP_CONTAINERS_LOADING_BODY,
        ),
        TableState::Stopped => (
            "icons/power.svg",
            gui::GUI_DESKTOP_CONTAINERS_ENGINE_STOPPED,
            gui::GUI_DESKTOP_CONTAINERS_ENGINE_STOPPED_BODY,
        ),
        TableState::Paused => (
            "icons/pause.svg",
            gui::GUI_DESKTOP_CONTAINERS_ENGINE_PAUSED,
            gui::GUI_DESKTOP_CONTAINERS_ENGINE_PAUSED_BODY,
        ),
        TableState::Offline | TableState::Failed => (
            "icons/unplug.svg",
            gui::GUI_DESKTOP_CONTAINERS_SERVICE_UNAVAILABLE,
            gui::GUI_DESKTOP_CONTAINERS_SERVICE_UNAVAILABLE_BODY,
        ),
        TableState::NoMatches => (
            "icons/search.svg",
            gui::GUI_DESKTOP_CONTAINERS_NO_MATCHES,
            gui::GUI_DESKTOP_CONTAINERS_NO_MATCHES_BODY,
        ),
        TableState::Empty | TableState::Rows => (
            "icons/box.svg",
            gui::GUI_DESKTOP_CONTAINERS_NONE,
            gui::GUI_DESKTOP_CONTAINERS_NONE_BODY,
        ),
    };
    let palette = screen.palette;
    let locale = screen.locale;
    let muted = color(palette, ColorToken::TextMuted);
    let mut content = div()
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .gap_3()
        .w_full()
        .pt_8()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(48.))
                .rounded(px(12.))
                .bg(color(
                    palette,
                    match state {
                        TableState::Offline | TableState::Failed => ColorToken::StatusWarningBg,
                        TableState::Loading | TableState::Stopped | TableState::Paused => {
                            ColorToken::AccentSubtle
                        }
                        _ => ColorToken::BgSubtle,
                    },
                ))
                .text_color(color(palette, ColorToken::TextPrimary))
                .child(Icon::empty().path(path)),
        )
        .child(
            screen
                .typography(Role::Section)
                .apply(div())
                .child(title.text(locale)),
        )
        .child(
            screen
                .typography(Role::Small)
                .apply(div())
                .text_color(muted)
                .child(detail.text(locale)),
        );
    match state {
        TableState::Stopped => {
            content = content.child(
                Button::new("start-engine-from-containers")
                    .primary()
                    .size(px(36.))
                    .p_0()
                    .icon(IconName::Play)
                    .accessibility_label(gui::GUI_DESKTOP_CONTAINERS_START_ENGINE.text(locale))
                    .tooltip(gui::GUI_DESKTOP_CONTAINERS_START_ENGINE.text(locale))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(StartEngineFromScreen), cx)
                    }),
            );
        }
        TableState::Offline | TableState::Failed => {
            content = content.child(
                Button::new("refresh-container-list")
                    .primary()
                    .size(px(36.))
                    .p_0()
                    .icon(IconName::RefreshCw)
                    .accessibility_label(gui::GUI_DESKTOP_RETRY.text(locale))
                    .tooltip(gui::GUI_DESKTOP_RETRY.text(locale))
                    .disabled(view.services.is_none())
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(services) = view.services.clone() {
                            cx.spawn(async move |view, cx| {
                                let mut refresh = services.refresh();
                                let _ = refresh.result().await;
                                let _ = view.update(cx, |_, cx| cx.notify());
                            })
                            .detach();
                        }
                    })),
            );
        }
        TableState::Empty => {
            const SAMPLE_COMMAND: &str = "docker run --rm hello-world";
            content = content.child(
                Button::new("copy-container-example")
                    .outline()
                    .label(isolate_technical_text(SAMPLE_COMMAND))
                    .accessibility_label(gui::GUI_DESKTOP_CONTAINERS_COPY_EXAMPLE.text(locale))
                    .on_click(|_, _, cx| {
                        cx.write_to_clipboard(marsa_ui::gpui::ClipboardItem::new_string(
                            SAMPLE_COMMAND.to_owned(),
                        ))
                    }),
            );
        }
        TableState::Loading => {
            content = content.child(div().flex().flex_col().gap_2().mt_2().w(px(232.)).children(
                [232., 188., 212.].into_iter().map(|width| {
                    div()
                        .w(px(width))
                        .h(px(7.))
                        .rounded_full()
                        .bg(color(palette, ColorToken::BgSubtle))
                }),
            ));
        }
        _ => {}
    }
    content
}
