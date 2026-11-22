//! Review and explicitly authorize the daemon's staged proxy activation.

use marsa_client::api::{
    ActivateProxyRequest, PrepareProxyActivationRequest, ProxyActivationOutcome,
};
use marsa_ui::{
    Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div},
    gpui_component::{Disableable, button::Button, switch::Switch},
    gui_keys as gui,
};

use crate::screens::common::{ScreenContext, message, row};

use super::super::{Notice, SystemView, machine::title_text};

impl SystemView {
    pub(super) fn proxy_controls(
        &self,
        screen: &ScreenContext<'_>,
        inject: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                row(locale)
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .child(title_text(gui::GUI_DESKTOP_PROXY_INJECTION.text(locale)))
                            .child(message(locale, gui::GUI_DESKTOP_PROXY_INJECTION_HELP)),
                    )
                    .child(
                        Switch::new("inject-container-proxy")
                            .accessibility_label(gui::GUI_DESKTOP_PROXY_INJECTION.text(locale))
                            .checked(inject)
                            .disabled(self.busy())
                            .on_change(cx.listener(|this, enabled: &bool, _, cx| {
                                this.save_settings(
                                    |settings| {
                                        settings.network.proxy.inject_into_containers = *enabled;
                                    },
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                row(locale).child(
                    Button::new("review-proxy-activation")
                        .outline()
                        .label(gui::GUI_DESKTOP_PROXY_REVIEW_AND_APPLY.text(locale))
                        .disabled(self.busy() || !self.restart_required)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.review_proxy(locale, window, cx);
                        })),
                ),
            )
    }

    fn review_proxy(&mut self, locale: Locale, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let (Some(service), Some((_, current_revision))) = (self.services.clone(), &self.settings)
        else {
            return;
        };
        let revision = *current_revision;
        let mut plan = service.request(move |client| async move {
            Ok(client
                .network()
                .prepare_proxy_activation(PrepareProxyActivationRequest {
                    expected_settings_revision: revision,
                })
                .await?
                .into_inner())
        });
        self.notice = Some(Notice::Working);
        self.command = Some(cx.spawn_in(window, async move |this, cx| {
            let Some(Ok(plan)) = plan.result().await else {
                let _ = this.update(cx, |this, cx| {
                    this.notice = Some(Notice::ProxyPreviousPolicy);
                    cx.notify();
                });
                return;
            };
            let answer = this.update_in(cx, |_, window, cx| {
                let impact = if plan.interrupts_running_engine {
                    gui::GUI_DESKTOP_PROXY_CONFIRM_RUNNING
                } else {
                    gui::GUI_DESKTOP_PROXY_CONFIRM_STOPPED
                };
                window.prompt(
                    PromptLevel::Warning,
                    gui::GUI_DESKTOP_PROXY_REVIEW_AND_APPLY.text(locale),
                    Some(impact.text(locale)),
                    &[
                        PromptButton::Ok(gui::GUI_DESKTOP_PROXY_APPLY.text(locale).into()),
                        PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
                    ],
                    cx,
                )
            });
            let Ok(answer) = answer else { return };
            if answer.await != Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    this.notice = None;
                    cx.notify();
                });
                return;
            }
            let Ok(request_id) = marsa_client::request_id() else {
                let _ = this.update(cx, |this, cx| {
                    this.notice = Some(Notice::Failed);
                    cx.notify();
                });
                return;
            };
            let mut pending = service.request(move |client| async move {
                Ok(client
                    .network()
                    .activate_proxy(ActivateProxyRequest {
                        expected_settings_revision: plan.settings_revision,
                        approval_token: plan.approval_token,
                        request_id,
                        approved: true,
                    })
                    .await?
                    .into_inner())
            });
            let notice = match pending.result().await {
                Some(Ok(result)) => match result.outcome() {
                    ProxyActivationOutcome::Applied => Notice::Saved,
                    ProxyActivationOutcome::AppliedDurabilityUncertain
                    | ProxyActivationOutcome::EngineRecoveryRequired => {
                        Notice::ProxyRecoveryRequired
                    }
                    _ => Notice::ProxyPreviousPolicy,
                },
                _ => Notice::ProxyPreviousPolicy,
            };
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(notice);
                if matches!(notice, Notice::Saved) {
                    this.fetch_settings(cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
