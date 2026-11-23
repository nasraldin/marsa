//! Show facts about the installed app and the connected local daemon.

use marsa_client::release::{AboutAssets, Evidence};
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement, Styled, div, px},
    gpui_component::button::Button,
    gui_keys as gui,
    tokens::{scale::Space, typography::Role},
};

use super::{SystemView, settings_card, settings_text};
use crate::screens::common::{ScreenContext, color, row, technical};

impl SystemView {
    pub(in crate::screens::system) fn load_about(&mut self, cx: &Context<Self>) {
        if self.about_assets.is_none() && self.about_assets_task.is_none() {
            let resources = cx.app_path().ok().map(|app| app.join("Contents/Resources"));
            let load = cx.background_executor().spawn(async move {
                resources.map_or_else(AboutAssets::default, |path| {
                    AboutAssets::read(&path, env!("CARGO_PKG_VERSION"))
                })
            });
            self.about_assets_task = Some(cx.spawn(async move |this, cx| {
                let assets = load.await;
                let _ = this.update(cx, |this, cx| {
                    this.about_assets = Some(assets);
                    this.about_assets_task = None;
                    cx.notify();
                });
            }));
        }

        self.load_component_versions(cx);
    }

    pub(super) fn load_component_versions(&mut self, cx: &Context<Self>) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        let mut pending = services.request(|client| async move { Ok(client.version().clone()) });
        self.route_load = Some(cx.spawn(async move |this, cx| {
            if let Some(Ok(version)) = pending.result().await {
                let _ = this.update(cx, |this, cx| {
                    this.version = Some(version);
                    cx.notify();
                });
            }
        }));
    }

    pub(super) fn about(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let assets = self.about_assets.as_ref();
        let release = assets.map_or_else(
            || gui::GUI_SETTINGS_ABOUT_LOADING.text(locale),
            |assets| {
                assets
                    .notes(locale)
                    .unwrap_or_else(|| gui::GUI_SETTINGS_ABOUT_NOTES_UNAVAILABLE.text(locale))
            },
        );

        let host = about_section(screen, gui::GUI_SETTINGS_ABOUT_DOCKER_TOOLS);
        let host = match assets {
            Some(assets) if !assets.host_tools().is_empty() => host.child(
                div()
                    .flex()
                    .flex_col()
                    .children(assets.host_tools().iter().map(|component| {
                        component_row(screen, technical(&component.name), &component.version)
                    })),
            ),
            Some(_) => host.child(muted(screen, gui::GUI_SETTINGS_ABOUT_TOOLS_UNAVAILABLE)),
            None => host.child(muted(screen, gui::GUI_SETTINGS_ABOUT_LOADING)),
        };
        let daemon = about_section(screen, gui::GUI_SETTINGS_ABOUT_DAEMON_COMPONENTS);
        let version = self
            .version
            .as_ref()
            .filter(|_| screen.state.daemon().is_some());
        let daemon = match version {
            Some(version) => daemon.child(
                div()
                    .flex()
                    .flex_col()
                    .child(component_row(
                        screen,
                        gui::GUI_DESKTOP_DAEMON.text(locale),
                        &version.daemon_version,
                    ))
                    .children(version.components.iter().map(|component| {
                        component_row(screen, technical(&component.name), &component.version)
                    })),
            ),
            None => daemon.child(muted(screen, gui::GUI_SETTINGS_ABOUT_DAEMON_UNAVAILABLE)),
        };

        let evidence = about_section(screen, gui::GUI_SETTINGS_ABOUT_EVIDENCE);
        let evidence = if let Some(assets) = assets {
            let sources = [
                (
                    Evidence::HostTools,
                    "about-host-inventory",
                    gui::GUI_SETTINGS_ABOUT_HOST_SBOM,
                ),
                (
                    Evidence::AppSbom,
                    "about-app-inventory",
                    gui::GUI_SETTINGS_ABOUT_APP_SBOM,
                ),
                (
                    Evidence::GuestSbom,
                    "about-guest-inventory",
                    gui::GUI_SETTINGS_ABOUT_GUEST_SBOM,
                ),
                (
                    Evidence::Notices,
                    "show-release-notices",
                    gui::GUI_DESKTOP_SHOW_RELEASE_NOTICES,
                ),
            ];
            let buttons: Vec<_> = sources
                .into_iter()
                .filter_map(|(kind, id, label)| {
                    let path = assets.evidence(kind)?.to_owned();
                    Some(
                        Button::new(id)
                            .outline()
                            .label(label.text(locale))
                            .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&path))),
                    )
                })
                .collect();
            if buttons.is_empty() {
                evidence.child(muted(screen, gui::GUI_SETTINGS_ABOUT_EVIDENCE_UNAVAILABLE))
            } else {
                let actions = row(locale)
                    .flex_wrap()
                    .gap(px(Space::S8.pixels()))
                    .children(buttons);
                let evidence = evidence.child(actions);
                if assets.evidence(Evidence::Notices).is_none() {
                    evidence.child(muted(screen, gui::GUI_SETTINGS_ABOUT_DISTRIBUTION_EVIDENCE))
                } else {
                    evidence
                }
            }
        } else {
            evidence.child(muted(screen, gui::GUI_SETTINGS_ABOUT_LOADING))
        };

        let release = settings_card(screen)
            .child(
                component_row(
                    screen,
                    gui::GUI_SETTINGS_ABOUT_VERSION.text(locale),
                    env!("CARGO_PKG_VERSION"),
                )
                .border_t_0(),
            )
            .child(muted(
                screen,
                gui::GUI_DESKTOP_MARSA_IS_RELEASED_UNDER_THE_MIT_LICENSE,
            ))
            .child(settings_text(
                screen,
                Role::Lead,
                gui::GUI_SETTINGS_ABOUT_RELEASE_NOTES.text(locale),
                ColorToken::TextPrimary,
            ))
            .child(settings_text(
                screen,
                Role::Body,
                release.to_owned(),
                ColorToken::TextPrimary,
            ));
        div()
            .flex()
            .flex_col()
            .gap(px(Space::S16.pixels()))
            .child(release)
            .child(host)
            .child(evidence)
            .child(daemon)
    }
}

fn about_section(screen: &ScreenContext<'_>, key: marsa_ui::TextKey) -> Div {
    settings_card(screen).child(settings_text(
        screen,
        Role::Lead,
        key.text(screen.locale),
        ColorToken::TextPrimary,
    ))
}

fn muted(screen: &ScreenContext<'_>, key: marsa_ui::TextKey) -> Div {
    settings_text(
        screen,
        Role::Small,
        key.text(screen.locale),
        ColorToken::TextMuted,
    )
}

fn component_row(
    screen: &ScreenContext<'_>,
    component: impl Into<marsa_ui::gpui::SharedString>,
    version: &str,
) -> Div {
    row(screen.locale)
        .justify_between()
        .gap(px(Space::S16.pixels()))
        .py(px(Space::S8.pixels()))
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(settings_text(screen, Role::Label, component, ColorToken::TextMuted).min_w_0())
        .child(settings_text(
            screen,
            Role::Mono,
            technical(version),
            ColorToken::TextPrimary,
        ))
}
