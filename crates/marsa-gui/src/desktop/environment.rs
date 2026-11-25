//! Present the selected external engine without invoking Marsa's VM controls.

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken,
    gpui::{
        Context, Div, ParentElement as _, Styled as _, div, prelude::FluentBuilder as _, px, svg,
    },
    gpui_component::{
        Disableable as _, IconName,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use crate::screens::{
    common::{ScreenContext, align, color, panel, row, technical},
    system::environment_icon_path,
};

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
        let workspace = self
            .external_inventory
            .iter()
            .find(|workspace| selection.includes_external(workspace));
        let engine = self.context_engine(cx);
        let busy = matches!(engine, Some(EngineState::Starting));
        let ready = engine == Some(EngineState::Ready);
        let system = self.system.read(cx);
        if route == Route::Machine {
            let name = workspace.map_or_else(
                || selection.name().to_owned(),
                |workspace| system.environment_presentation(Some(workspace.as_ref())).0,
            );
            let connection = self.context_status_text(engine, cx);
            return self.system.update(cx, |system, cx| {
                system.external_machine(&name, workspace.map(AsRef::as_ref), connection, screen, cx)
            });
        }
        let identity = if let Some(workspace) = workspace {
            let (name, icon) = system.environment_presentation(Some(workspace.as_ref()));
            (technical(&name), environment_icon_path(icon))
        } else {
            (selection.display(), Route::Machine.icon())
        };

        let heading = row(locale)
            .gap_3()
            .flex_wrap()
            .child(
                svg()
                    .path(identity.1)
                    .size(px(20.))
                    .text_color(color(palette, ColorToken::AccentDefault)),
            )
            .child(
                screen
                    .typography(Role::Title)
                    .apply(div().min_w_0().truncate())
                    .child(identity.0),
            )
            .child(status::inline_badge(
                locale,
                palette,
                engine,
                self.context_status_text(engine, cx),
            ));

        let refresh = Button::new("refresh-active-environment")
            .outline()
            .icon(IconName::RefreshCw)
            .label(gui::GUI_DESKTOP_REFRESH.text(locale))
            .disabled(busy)
            .on_click(cx.listener(|desktop, _, _, cx| {
                desktop
                    .system
                    .update(cx, |system, cx| system.refresh_external_inventory(cx));
            }));
        let manage = Button::new("manage-active-environment")
            .primary()
            .label(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
            .on_click(cx.listener(|desktop, _, _, cx| desktop.open_contexts(cx)));

        align(div().flex().flex_col().gap_4(), locale).child(
            panel(screen)
                .gap_4()
                .child(
                    row(locale)
                        .justify_between()
                        .gap_3()
                        .flex_wrap()
                        .child(heading)
                        .child(row(locale).gap_2().flex_wrap().child(refresh).child(manage)),
                )
                .when_some(workspace, |card, workspace| {
                    card.child(
                        row(locale)
                            .gap_3()
                            .flex_wrap()
                            .child(
                                screen
                                    .typography(Role::Mono)
                                    .apply(div().min_w_0().truncate())
                                    .child(technical(&workspace.endpoint.address())),
                            )
                            .children(workspace.verified_engine.as_ref().map(|engine| {
                                screen
                                    .typography(Role::Small)
                                    .apply(div())
                                    .text_color(color(palette, ColorToken::TextMuted))
                                    .child(technical(&format!("Docker {}", engine.version)))
                            })),
                    )
                })
                .when(!ready && !busy, |card| {
                    card.child(
                        screen
                            .typography(Role::Small)
                            .apply(div())
                            .text_color(color(palette, ColorToken::StatusWarning))
                            .child(gui::GUI_ENVIRONMENT_OFFLINE.text(locale)),
                    )
                })
                .when(ready && route == Route::Disk, |card| {
                    card.child(
                        row(locale)
                            .gap_3()
                            .flex_wrap()
                            .border_t_1()
                            .border_color(color(palette, ColorToken::BorderSubtle))
                            .pt_3()
                            .child(
                                screen
                                    .typography(Role::Small)
                                    .apply(div().flex_1().min_w(px(200.)))
                                    .text_color(color(palette, ColorToken::TextMuted))
                                    .child(gui::GUI_ENVIRONMENT_DISK_HELP.text(locale)),
                            )
                            .child(
                                Button::new("external-disk-open-volumes")
                                    .outline()
                                    .label(Route::Volumes.name(locale))
                                    .on_click(cx.listener(|desktop, _, _, cx| {
                                        desktop.navigate(Route::Volumes, cx)
                                    })),
                            ),
                    )
                }),
        )
    }
}
