//! Present the selected external engine without invoking Marsa's VM controls.

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement as _, Styled as _, div, prelude::FluentBuilder as _, px},
    gpui_component::button::{Button, ButtonVariants as _},
    gui_keys as gui,
    tokens::typography::Role,
};

use crate::screens::common::{ScreenContext, align, color, row, technical};

use super::{Desktop, Route, status};

impl Desktop {
    pub(super) fn external_screen(
        &self,
        route: Route,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let selection = self.environment.read(cx);
        let workspace = self.active_workspace(cx);
        let engine = self.context_engine(cx);
        let card = |label| {
            align(div().flex().flex_col().gap_2(), locale)
                .min_w(px(138.))
                .rounded(px(10.))
                .border_1()
                .border_color(color(palette, ColorToken::BorderSubtle))
                .bg(color(palette, ColorToken::BgSidebar))
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(color(palette, ColorToken::TextMuted))
                        .child(label),
                )
        };
        let connection = card(gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale))
            .child(
                screen
                    .typography(Role::Title)
                    .apply(div())
                    .child(workspace.map_or_else(
                        || selection.display(),
                        |workspace| technical(&workspace.source),
                    )),
            )
            .child(status::inline_badge(
                locale,
                palette,
                engine,
                self.context_status_text(engine, cx),
            ))
            .when_some(workspace, |card, workspace| {
                card.child(
                    div()
                        .text_sm()
                        .child(technical(&workspace.endpoint.address())),
                )
            });
        let refresh = Button::new("refresh-active-environment")
            .outline()
            .label(gui::GUI_DESKTOP_REFRESH.text(locale))
            .on_click(cx.listener(|desktop, _, _, cx| {
                desktop
                    .system
                    .update(cx, |system, cx| system.refresh_external_inventory(cx));
            }));
        let mut body = align(div().flex().flex_col().gap_4().p_4(), locale)
            .child(connection)
            .child(
                row(locale).gap_2().child(refresh).child(
                    Button::new("manage-active-environment")
                        .ghost()
                        .label(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
                        .on_click(cx.listener(|desktop, _, _, cx| desktop.open_contexts(cx))),
                ),
            );
        if workspace.is_none() {
            return if engine == Some(EngineState::Starting) {
                body
            } else {
                body.child(gui::GUI_ENVIRONMENT_OFFLINE.text(locale))
            };
        }
        if route == Route::Disk {
            body = body
                .child(gui::GUI_ENVIRONMENT_DISK_HELP.text(locale))
                .child(
                    Button::new("external-disk-open-volumes")
                        .outline()
                        .label(Route::Volumes.name(locale))
                        .on_click(
                            cx.listener(|desktop, _, _, cx| desktop.navigate(Route::Volumes, cx)),
                        ),
                );
        }
        body
    }
}
