//! Docker integration and Keychain settings.

use crate::screens::common::message;
use marsa_client::api::{
    EnableRegistryKeychainRequest, EnsureDockerContextRequest, GetDockerEndpointRequest,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window},
    gpui_component::{Disableable, button::Button},
    tokens::typography::Role,
};

use super::{
    super::{Notice, SystemView},
    settings_card, settings_heading, settings_text,
};
use crate::screens::common::{ScreenContext, row, technical};

impl SystemView {
    pub(super) fn docker_cli_integration(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        settings_card(screen)
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
            )
    }

    pub(super) fn registry_keychain(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        settings_card(screen)
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
}
