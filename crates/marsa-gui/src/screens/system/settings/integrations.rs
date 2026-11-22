//! Docker integration, release preferences, and component information.

use crate::screens::common::message;
use marsa_client::api::{
    EnableRegistryKeychainRequest, EnsureDockerContextRequest, GetDockerEndpointRequest,
    VersionResponse,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div, px},
    gpui_component::{Disableable, button::Button},
    tokens::{scale::Space, typography::Role},
};

use super::{
    super::{Notice, SystemView},
    settings_card, settings_heading, settings_text,
};
use crate::screens::common::{ScreenContext, color, row, technical};

impl SystemView {
    pub(super) fn docker(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let context = settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale),
                gui::GUI_DESKTOP_REGISTER_THE_LOCAL_MARSA_CONTEXT_WITH_DOCKER.text(locale),
            ))
            .child(settings_text(
                screen,
                Role::Mono,
                technical("marsa"),
                ColorToken::TextPrimary,
            ))
            .child(
                row(screen.locale)
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("register-docker-context")
                            .outline()
                            .label(message(screen.locale, gui::GUI_DESKTOP_REGISTER_CONTEXT))
                            .disabled(self.busy() || screen.state.daemon().is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let Some(request_id) = this.mutation_id(cx) else {
                                    return;
                                };
                                this.request(
                                    move |client| async move {
                                        Ok(client
                                            .docker_discovery()
                                            .ensure_context(EnsureDockerContextRequest {
                                                request_id,
                                            })
                                            .await?
                                            .into_inner())
                                    },
                                    |this, _, _| this.notice = Some(Notice::Saved),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("copy-docker-endpoint")
                            .outline()
                            .label(message(
                                screen.locale,
                                gui::GUI_DESKTOP_COPY_DOCKER_ENDPOINT,
                            ))
                            .disabled(self.busy() || screen.state.daemon().is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.request(
                                    |client| async move {
                                        Ok(client
                                            .docker_discovery()
                                            .get_endpoint(GetDockerEndpointRequest {})
                                            .await?
                                            .into_inner())
                                    },
                                    |_, endpoint, cx| {
                                        cx.write_to_clipboard(
                                            marsa_ui::gpui::ClipboardItem::new_string(
                                                endpoint.docker_host,
                                            ),
                                        );
                                    },
                                    cx,
                                );
                            })),
                    ),
            );
        let registry = settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_REGISTRY_CREDENTIALS.text(locale),
                gui::GUI_DESKTOP_USE_MACOS_KEYCHAIN_FOR_DOCKER_REGISTRY_SIGN_INS.text(locale),
            ))
            .child(
                row(locale).child(
                    Button::new("enable-registry-keychain")
                        .outline()
                        .disabled(self.busy() || screen.state.daemon().is_none())
                        .label(message(screen.locale, gui::GUI_DESKTOP_ENABLE_KEYCHAIN))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.confirm_keychain(locale, window, cx)
                        })),
                ),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(Space::S16.pixels() + Space::S4.pixels()))
            .child(context)
            .child(registry)
    }

    fn confirm_keychain(&mut self, locale: Locale, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Info,
            gui::GUI_DESKTOP_USE_KEYCHAIN_FOR_DOCKER_REGISTRY_SIGN_INS.text(locale),
            Some(
                gui::GUI_DESKTOP_MARSA_WILL_CONFIGURE_DOCKER_TO_USE_THE_MACOS_CREDENTIAL_HELPER
                    .text(locale),
            ),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_ENABLE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    let Some(request_id) = this.mutation_id(cx) else {
                        return;
                    };
                    this.request(
                        move |client| async move {
                            Ok(client
                                .docker_discovery()
                                .enable_registry_keychain(EnableRegistryKeychainRequest {
                                    request_id,
                                })
                                .await?
                                .into_inner())
                        },
                        |this, _, _| this.notice = Some(Notice::Saved),
                        cx,
                    );
                });
            }
        }));
    }

    pub(in crate::screens::system) fn load_version(&mut self, cx: &Context<Self>) {
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
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_MARSA.text(locale),
                gui::GUI_DESKTOP_MARSA_IS_RELEASED_UNDER_THE_MIT_LICENSE.text(locale),
            ))
            .child(
                row(locale).child(
                    Button::new("show-release-notices")
                        .outline()
                        .disabled(self.busy())
                        .label(gui::GUI_DESKTOP_SHOW_RELEASE_NOTICES.text(locale))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reveal_release_notices(locale, window, cx);
                        })),
                ),
            )
            .child(component_row(
                screen,
                gui::GUI_DESKTOP_MARSA.text(locale),
                env!("CARGO_PKG_VERSION"),
            ))
            .children(self.version.as_ref().map(
                |VersionResponse {
                     daemon_version,
                     components,
                     ..
                 }| {
                    div()
                        .flex()
                        .flex_col()
                        .child(component_row(
                            screen,
                            gui::GUI_DESKTOP_DAEMON.text(locale),
                            daemon_version,
                        ))
                        .children(components.iter().map(|component| {
                            component_row(screen, &component.name, &component.version)
                        }))
                },
            ))
    }

    fn reveal_release_notices(
        &mut self,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = cx
            .app_path()
            .ok()
            .map(|app| app.join("Contents/Resources/licenses/THIRD-PARTY-NOTICES.txt"));
        let check = cx
            .background_executor()
            .spawn(async move { path.filter(|path| path.is_file()) });
        cx.spawn_in(window, async move |_, cx| {
            let path = check.await;
            let _ = cx.update(|window, cx| {
                if let Some(path) = path {
                    cx.reveal_path(&path);
                } else {
                    drop(window.prompt(
                        PromptLevel::Info,
                        gui::GUI_DESKTOP_RELEASE_NOTICES_UNAVAILABLE.text(locale),
                        Some(gui::GUI_DESKTOP_RELEASE_NOTICES_USE_SIGNED_RELEASE.text(locale)),
                        &[PromptButton::Ok(gui::GUI_DESKTOP_CLOSE.text(locale).into())],
                        cx,
                    ));
                }
            });
        })
        .detach();
    }
}

fn component_row(screen: &ScreenContext<'_>, component: &str, version: &str) -> Div {
    row(screen.locale)
        .justify_between()
        .gap(px(Space::S16.pixels()))
        .py(px(Space::S8.pixels()))
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(settings_text(
            screen,
            Role::Label,
            component.to_owned(),
            ColorToken::TextMuted,
        ))
        .child(settings_text(
            screen,
            Role::Mono,
            technical(version),
            ColorToken::TextPrimary,
        ))
}
