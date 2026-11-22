//! Docker integration, release preferences, and component information.

use crate::screens::common::message;
use marsa_client::api::{
    EnableRegistryKeychainRequest, EnsureDockerContextRequest, GetDockerEndpointRequest,
    VersionResponse,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window},
    gpui_component::{Disableable, button::Button},
};

use super::super::{Notice, SystemView, machine::title_text};
use crate::screens::common::{ScreenContext, panel, row, technical};

impl SystemView {
    pub(super) fn docker(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        panel(screen)
            .child(title_text(
                gui::GUI_DESKTOP_DOCKER_CONTEXT.text(screen.locale),
            ))
            .child(technical("marsa"))
            .child(message(
                screen.locale,
                gui::GUI_DESKTOP_REGISTER_THE_LOCAL_MARSA_CONTEXT_WITH_DOCKER,
            ))
            .child(
                row(screen.locale)
                    .gap_2()
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
            )
            .child(title_text(
                gui::GUI_DESKTOP_REGISTRY_CREDENTIALS.text(screen.locale),
            ))
            .child(message(
                screen.locale,
                gui::GUI_DESKTOP_USE_MACOS_KEYCHAIN_FOR_DOCKER_REGISTRY_SIGN_INS,
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
            )
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

    pub(super) fn about(&self, screen: &ScreenContext<'_>) -> Div {
        let version = self.version.as_ref();
        panel(screen)
            .child(title_text(gui::GUI_DESKTOP_MARSA.text(screen.locale)))
            .child(technical(env!("CARGO_PKG_VERSION")))
            .children(version.map(|VersionResponse { daemon_version, .. }| {
                row(screen.locale)
                    .gap_2()
                    .child(message(screen.locale, gui::GUI_DESKTOP_DAEMON))
                    .child(technical(daemon_version))
            }))
            .children(
                version
                    .into_iter()
                    .flat_map(|value| &value.components)
                    .map(|component| {
                        row(screen.locale)
                            .justify_between()
                            .child(technical(&component.name))
                            .child(technical(&component.version))
                    }),
            )
            .child(message(
                screen.locale,
                gui::GUI_DESKTOP_MARSA_IS_RELEASED_UNDER_THE_MIT_LICENSE,
            ))
    }
}
