//! Confirm agent decisions and project grant revocation through the owner API.

use crate::screens::common::message;
use marsa_client::api::{
    ApproveRequest, DenyRequest, PendingApproval, PermissionTier, RevokeGrantRequest,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div},
    gpui_component::{Disableable, button::Button},
};

use super::{Notice, SystemView, disk::bytes};
use crate::screens::common::{ScreenContext, color, row, technical};

impl SystemView {
    pub(super) fn approval(
        &self,
        screen: &ScreenContext<'_>,
        approval: &PendingApproval,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let choices = [
            (
                Some(false),
                message(screen.locale, gui::GUI_DESKTOP_APPROVE_ONCE),
            ),
            (
                Some(true),
                message(screen.locale, gui::GUI_DESKTOP_ALLOW_FOR_THIS_PROJECT),
            ),
            (None, message(screen.locale, gui::GUI_DESKTOP_DENY)),
        ];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .py_2()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .child(
                row(locale)
                    .gap_3()
                    .child(technical(&approval.client_name))
                    .child(technical(approval.operation().as_str_name())),
            )
            .child(technical(&approval.project_path))
            .child(technical(&approval.redacted_arguments_json))
            .children(approval.affected.iter().map(|resource| {
                row(locale)
                    .gap_2()
                    .child(technical(&resource.kind))
                    .child(technical(&resource.name))
                    .children(resource.size_bytes.map(|size| technical(&bytes(size))))
                    .children(
                        resource
                            .related_containers
                            .iter()
                            .map(|name| technical(name)),
                    )
            }))
            .child(
                row(locale).gap_2().children(
                    choices
                        .into_iter()
                        .filter(|(choice, _)| {
                            *choice != Some(true)
                                || (approval.can_grant_for_project
                                    && approval.tier() == PermissionTier::Operate)
                        })
                        .enumerate()
                        .map(|(index, (choice, text))| {
                            let approval = approval.clone();
                            Button::new(format!("approval-{}-{index}", approval.id))
                                .outline()
                                .disabled(self.busy())
                                .label(text)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_approval(&approval, choice, locale, window, cx)
                                }))
                        }),
                ),
            )
    }

    fn confirm_approval(
        &mut self,
        approval: &PendingApproval,
        choice: Option<bool>,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let id = approval.id.clone();
        let message = match choice {
            Some(true) => gui::GUI_DESKTOP_GRANT_THIS_AGENT_ACCESS_FOR_THIS_PROJECT.text(locale),
            Some(false) => gui::GUI_DESKTOP_APPROVE_THIS_AGENT_REQUEST_ONCE.text(locale),
            None => gui::GUI_DESKTOP_DENY_THIS_AGENT_REQUEST.text(locale),
        };
        let detail = technical(&format!(
            "{}\n{}\n{}",
            approval.client_name,
            approval.project_path,
            approval.operation().as_str_name()
        ));
        let answer = window.prompt(
            PromptLevel::Warning,
            message,
            Some(detail.as_ref()),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_CONFIRM.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.resolve_approval(id, choice, cx));
            }
        }));
    }

    fn resolve_approval(
        &mut self,
        approval_id: String,
        choice: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.request(
            move |client| async move {
                let mut agents = client.agents();
                Ok(match choice {
                    Some(grant_for_project) => {
                        agents
                            .approve(ApproveRequest {
                                request_id,
                                approval_id,
                                grant_for_project,
                            })
                            .await?
                    }
                    None => {
                        agents
                            .deny(DenyRequest {
                                request_id,
                                approval_id,
                            })
                            .await?
                    }
                }
                .into_inner())
            },
            |this, _, cx| {
                this.notice = Some(Notice::Saved);
                this.load_audit(cx);
            },
            cx,
        );
    }

    pub(super) fn confirm_revoke(
        &mut self,
        grant_id: String,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_DESKTOP_REVOKE_THIS_PROJECT_GRANT.text(locale),
            None,
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_REVOKE.text(locale).into()),
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
                                .agents()
                                .revoke_grant(RevokeGrantRequest {
                                    request_id,
                                    grant_id,
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
