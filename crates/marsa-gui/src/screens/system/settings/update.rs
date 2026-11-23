//! Present authenticated update metadata and dispatch explicit owner actions.

use marsa_client::{
    ClientError,
    api::{
        ApplyUpdateRequest, CheckUpdatesRequest, DownloadUpdateRequest, UpdatePhase, UpdateStatus,
        UpdateStatusRequest,
    },
};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div},
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants},
        checkbox::Checkbox,
    },
    gui_keys as gui,
    preferences::UpdateChannel,
    tokens::typography::Role,
};

use super::{
    super::{SystemView, disk::bytes},
    settings_card, settings_text,
};
use crate::{
    screens::common::{ScreenContext, color, message, row, technical},
    services::ServiceError,
};

#[derive(Default)]
pub(in crate::screens::system) struct UpdatePanel {
    availability: Availability,
    status: Option<UpdateStatus>,
}

#[derive(Default, Eq, PartialEq)]
enum Availability {
    #[default]
    Pending,
    Available,
    Absent,
    Failed,
}

impl SystemView {
    pub(in crate::screens::system) fn load_update_status(&mut self, cx: &mut Context<Self>) {
        let Some(services) = &self.services else {
            return;
        };
        let mut response = services.request(|client| async move {
            Ok(client
                .updates()
                .status(UpdateStatusRequest {})
                .await?
                .into_inner())
        });
        self.route_load = Some(cx.spawn(async move |this, cx| {
            let result = response.result().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Some(Ok(status)) => this.update_panel.observe(status),
                    Some(Err(ServiceError::Daemon(error))) if error.is_unimplemented() => {
                        this.update_panel.availability = Availability::Absent;
                        this.update_panel.status = None;
                    }
                    _ => this.update_panel.availability = Availability::Failed,
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn updates(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let event = screen
            .state
            .daemon()
            .and_then(|snapshot| snapshot.update.as_ref());
        let status = event.or(self.update_panel.status.as_ref());
        let online = screen.state.daemon().is_some();
        let supported = online
            && (event.is_some() || self.update_panel.availability == Availability::Available);
        let channel = self
            .settings
            .as_ref()
            .map(|(settings, _)| settings.updates.channel);
        let auto_check = self
            .settings
            .as_ref()
            .is_some_and(|(settings, _)| settings.updates.auto_check);
        let configured = channel.is_some() && online;
        let phase = status.map_or(UpdatePhase::Unspecified, |status| status.phase());
        let can_check = supported
            && !self.busy()
            && matches!(
                phase,
                UpdatePhase::Unspecified
                    | UpdatePhase::Idle
                    | UpdatePhase::Failed
                    | UpdatePhase::RolledBack
                    | UpdatePhase::Installed
            );
        let version = status
            .and_then(|status| status.available_version.as_deref())
            .filter(|version| valid_version(version));
        let preferences = div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .child(
                row(locale)
                    .gap_2()
                    .flex_wrap()
                    .justify_between()
                    .child(settings_text(
                        screen,
                        Role::Label,
                        gui::GUI_UPDATE_RELEASE_CHANNEL.text(locale),
                        ColorToken::TextPrimary,
                    ))
                    .child(
                        row(locale).gap_2().children(
                            [
                                (UpdateChannel::Stable, gui::GUI_DESKTOP_STABLE),
                                (UpdateChannel::Beta, gui::GUI_DESKTOP_BETA),
                            ]
                            .into_iter()
                            .map(|(value, key)| {
                                Button::new(("release-channel", value as usize))
                                    .outline()
                                    .selected(channel == Some(value))
                                    .toggled(channel == Some(value))
                                    .label(key.text(locale))
                                    .disabled(self.busy() || !configured)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.save_settings(
                                            |settings| settings.updates.channel = value,
                                            cx,
                                        );
                                    }))
                            }),
                        ),
                    ),
            )
            .child(
                Checkbox::new("automatic-update-checks")
                    .checked(auto_check)
                    .disabled(self.busy() || !configured)
                    .label(message(locale, gui::GUI_UPDATE_AUTO_CHECK))
                    .on_click(cx.listener(|this, selected: &bool, _, cx| {
                        this.save_settings(|settings| settings.updates.auto_check = *selected, cx);
                    })),
            );
        settings_card(screen)
            .child(settings_text(
                screen,
                Role::Lead,
                gui::GUI_DESKTOP_UPDATES.text(locale),
                ColorToken::TextPrimary,
            ))
            .children(status.map(|status| summary(status, locale)))
            .children((!supported).then(|| {
                message(
                    locale,
                    match self.update_panel.availability {
                        Availability::Pending => gui::GUI_UPDATE_LOADING,
                        Availability::Failed => gui::GUI_UPDATE_RETRY_STATUS,
                        _ => gui::GUI_DESKTOP_THIS_MARSA_BUILD_DOES_NOT_OFFER_IN_APP_UPDATES,
                    },
                )
            }))
            .child(
                row(locale)
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("check-updates")
                            .outline()
                            .label(gui::GUI_DESKTOP_CHECK_FOR_UPDATES.text(locale))
                            .disabled(!can_check)
                            .on_click(cx.listener(|this, _, _, cx| this.check_update(cx))),
                    )
                    .children(
                        (online && self.update_panel.availability == Availability::Failed).then(
                            || {
                                Button::new("retry-updater-status")
                                    .outline()
                                    .label(gui::GUI_DESKTOP_RETRY.text(locale))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.load_update_status(cx)),
                                    )
                            },
                        ),
                    )
                    .children(
                        version
                            .filter(|_| phase == UpdatePhase::Available)
                            .map(|version| {
                                let version = version.to_owned();
                                Button::new("download-update")
                                    .primary()
                                    .label(gui::GUI_DESKTOP_DOWNLOAD.text(locale))
                                    .disabled(self.busy() || !supported)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.download_update(version.clone(), cx)
                                    }))
                            }),
                    )
                    .children(version.filter(|_| phase == UpdatePhase::ReadyToApply).map(
                        |version| {
                            let version = version.to_owned();
                            Button::new("install-update")
                                .primary()
                                .label(gui::GUI_UPDATE_INSTALL.text(locale))
                                .disabled(self.busy() || !supported)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_update(version.clone(), locale, window, cx);
                                }))
                        },
                    )),
            )
            .child(preferences)
    }

    fn check_update(&mut self, cx: &mut Context<Self>) {
        self.run_update(
            |client| async move {
                Ok(client
                    .updates()
                    .check(CheckUpdatesRequest {})
                    .await?
                    .into_inner())
            },
            cx,
        );
    }

    fn download_update(&mut self, version: String, cx: &mut Context<Self>) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.run_update(
            move |client| async move {
                Ok(client
                    .updates()
                    .download(DownloadUpdateRequest {
                        request_id,
                        version,
                    })
                    .await?
                    .into_inner())
            },
            cx,
        );
    }

    fn confirm_update(
        &mut self,
        version: String,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_UPDATE_CONFIRM_INSTALL.text(locale),
            Some(gui::GUI_UPDATE_WORKLOAD_WARNING.text(locale)),
            &[
                PromptButton::Ok(gui::GUI_UPDATE_INSTALL.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let Some(request_id) = this.mutation_id(cx) else {
                    return;
                };
                this.run_update(
                    move |client| async move {
                        Ok(client
                            .updates()
                            .apply(ApplyUpdateRequest {
                                request_id,
                                version,
                            })
                            .await?
                            .into_inner())
                    },
                    cx,
                );
            });
        }));
    }

    fn run_update<F>(
        &mut self,
        operation: impl FnOnce(marsa_client::MarsaClient) -> F + Send + 'static,
        cx: &mut Context<Self>,
    ) where
        F: Future<Output = Result<UpdateStatus, ClientError>> + Send + 'static,
    {
        self.request(
            operation,
            |this, status, _| {
                this.update_panel.observe(status);
            },
            cx,
        );
    }
}

impl UpdatePanel {
    fn observe(&mut self, status: UpdateStatus) {
        self.availability = Availability::Available;
        self.status = Some(status);
    }
}

fn summary(status: &UpdateStatus, locale: Locale) -> Div {
    let phase = match status.phase() {
        UpdatePhase::Checking => gui::GUI_UPDATE_CHECKING,
        UpdatePhase::Available => gui::GUI_UPDATE_AVAILABLE,
        UpdatePhase::Downloading => gui::GUI_UPDATE_DOWNLOADING,
        UpdatePhase::ReadyToApply => gui::GUI_UPDATE_READY,
        UpdatePhase::Applying => gui::GUI_UPDATE_INSTALLING,
        UpdatePhase::Failed => gui::GUI_UPDATE_FAILED,
        UpdatePhase::RolledBack => gui::GUI_UPDATE_ROLLED_BACK,
        UpdatePhase::Installed => gui::GUI_UPDATE_INSTALLED,
        _ => gui::GUI_UPDATE_IDLE,
    };
    let notes = match locale {
        Locale::EnglishUs => status.notes_en_us.as_deref(),
        Locale::Arabic => status.notes_ar.as_deref().or(status.notes_en_us.as_deref()),
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            row(locale).gap_2().child(phase.text(locale)).children(
                status
                    .available_version
                    .as_deref()
                    .filter(|version| valid_version(version))
                    .map(technical),
            ),
        )
        .children(
            status
                .security_recommended
                .then(|| gui::GUI_UPDATE_SECURITY.text(locale)),
        )
        .children(status.bytes_downloaded.map(|downloaded| {
            row(locale)
                .gap_2()
                .child(technical(&bytes(downloaded)))
                .children(
                    status
                        .download_size_bytes
                        .map(|total| technical(&bytes(total))),
                )
        }))
        .children(notes.filter(|notes| notes.len() <= 8192).map(|notes| {
            div().child(
                notes
                    .chars()
                    .filter(|character| {
                        !matches!(character,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' |
                '\u{2066}'..='\u{2069}')
                    })
                    .collect::<String>(),
            )
        }))
}

fn valid_version(version: &str) -> bool {
    version.len() <= 32
        && version.split('.').count() == 3
        && version.split('.').all(|part| {
            (part == "0" || !part.starts_with('0'))
                && part.parse::<u32>().is_ok()
                && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}
