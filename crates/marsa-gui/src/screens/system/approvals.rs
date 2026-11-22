//! Confirm agent decisions and project grant revocation through the owner API.

use marsa_client::api::{
    ApproveRequest, DenyRequest, PendingApproval, PermissionTier, RevokeGrantRequest,
};
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        AppContext, Context, Div, ParentElement, PromptButton, PromptLevel, Styled, Window, div, px,
    },
    gpui_component::{
        Disableable, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
    },
};

use super::{
    Notice, SystemView,
    disk::bytes,
    machine::{dialog_title, mono, muted, system_icon},
};
use crate::screens::common::{ScreenContext, align, color, row};

impl SystemView {
    pub(super) fn approval(
        &self,
        screen: &ScreenContext<'_>,
        approval: &PendingApproval,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let request = approval.clone();
        row(locale)
            .min_h(px(53.))
            .gap_3()
            .border_t_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .child(
                div()
                    .w(px(112.))
                    .text_color(color(palette, ColorToken::StatusStarting))
                    .child(gui::GUI_SYSTEM_ACTIVITY_PENDING.text(locale)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(mono(&approval.client_name))
                    .child(
                        mono(approval.operation().as_str_name())
                            .text_color(color(palette, ColorToken::TextMuted)),
                    ),
            )
            .child(
                Button::new(format!("review-agent-request-{}", approval.id))
                    .outline()
                    .small()
                    .disabled(self.busy())
                    .label(gui::GUI_SYSTEM_ACTIVITY_REVIEW.text(locale))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.review_approval(request.clone(), locale, palette, window, cx)
                    })),
            )
    }

    fn review_approval(
        &self,
        approval: PendingApproval,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() || window.has_active_dialog(cx) {
            return;
        }
        let destroy = approval.tier() == PermissionTier::Destroy;
        let volume = approval
            .affected
            .iter()
            .any(|resource| resource.kind == "volume");
        let can_grant =
            approval.can_grant_for_project && approval.tier() == PermissionTier::Operate;
        let confirmation = destroy.then(|| match approval.affected.as_slice() {
            [resource] if !resource.name.is_empty() => resource.name.clone(),
            _ => approval.id.clone(),
        });
        let input = confirmation.as_ref().map(|_| {
            let input = cx.new(|cx| InputState::new(window, cx));
            cx.subscribe_in(&input, window, |_, _, event, window, _| {
                if matches!(event, InputEvent::Change) {
                    window.refresh();
                }
            })
            .detach();
            input
        });
        let title = if volume && destroy {
            gui::GUI_SYSTEM_ACTIVITY_DESTROY_VOLUME_TITLE
        } else if destroy {
            gui::GUI_SYSTEM_ACTIVITY_DESTROY_TITLE
        } else {
            gui::GUI_SYSTEM_ACTIVITY_OPERATE_TITLE
        };
        let intro = if destroy {
            gui::GUI_SYSTEM_ACTIVITY_DESTROY_EXPLANATION
        } else {
            gui::GUI_SYSTEM_ACTIVITY_REVIEW_EXACT
        };
        let confirmation_key = if volume {
            gui::GUI_SYSTEM_ACTIVITY_TYPE_VOLUME
        } else {
            gui::GUI_SYSTEM_ACTIVITY_TYPE_TARGET
        };
        let view = cx.entity().downgrade();
        let approval_id = approval.id.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted_color = color(palette, ColorToken::TextMuted);
            let mut content = align(div().flex().flex_col().gap_4(), locale)
                .child(muted(palette, intro.text(locale)))
                .child(
                    row(locale)
                        .justify_between()
                        .gap_3()
                        .child(approval_fact(
                            locale,
                            palette,
                            gui::GUI_SYSTEM_ACTIVITY_REQUESTED_BY,
                            &approval.client_name,
                        ))
                        .child(approval_fact(
                            locale,
                            palette,
                            gui::GUI_SYSTEM_ACTIVITY_PROJECT,
                            &approval.project_path,
                        ))
                        .child(permission_pill(locale, palette, approval.tier())),
                );
            if destroy {
                let resources = div()
                    .p_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(color(palette, ColorToken::BorderSubtle))
                    .bg(color(palette, ColorToken::BgCode))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(approval.affected.iter().map(|resource| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(approval_detail(
                                locale,
                                palette,
                                if resource.kind == "volume" {
                                    gui::GUI_SYSTEM_ACTIVITY_VOLUME
                                } else {
                                    gui::GUI_SYSTEM_ACTIVITY_TARGET
                                },
                                &resource.name,
                            ))
                            .children(resource.size_bytes.map(|size| {
                                approval_detail(
                                    locale,
                                    palette,
                                    gui::GUI_SYSTEM_ACTIVITY_SIZE,
                                    &bytes(size),
                                )
                            }))
                            .children((!resource.related_containers.is_empty()).then(|| {
                                approval_detail(
                                    locale,
                                    palette,
                                    gui::GUI_SYSTEM_ACTIVITY_PREVIOUSLY_USED,
                                    &resource.related_containers.join(", "),
                                )
                            }))
                    }));
                if !approval.affected.is_empty() {
                    content = content.child(resources);
                }
            } else {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(gui::GUI_SYSTEM_ACTIVITY_OPERATION_DETAILS.text(locale))
                        .child(
                            div()
                                .p_3()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(color(palette, ColorToken::BorderSubtle))
                                .bg(color(palette, ColorToken::BgCode))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(mono(approval.operation().as_str_name()))
                                .children(
                                    (!approval.redacted_arguments_json.is_empty())
                                        .then(|| mono(&approval.redacted_arguments_json)),
                                ),
                        ),
                );
            }
            if let (Some(input), Some(phrase)) = (&input, &confirmation) {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(confirmation_key.text(locale))
                        .child(Input::new(input).aria_label(confirmation_key.text(locale)))
                        .child(mono(phrase).text_color(muted_color)),
                );
            }
            if approval.expires_at_unix_millis > 0 {
                content = content.child(
                    row(locale)
                        .gap_2()
                        .child(muted(
                            palette,
                            gui::GUI_SYSTEM_ACTIVITY_EXPIRES_UTC.text(locale),
                        ))
                        .child(
                            mono(&super::activity::utc_time(approval.expires_at_unix_millis))
                                .text_color(muted_color),
                        ),
                );
            }
            if can_grant {
                content = content.child(
                    row(locale)
                        .items_start()
                        .gap_3()
                        .p_4()
                        .rounded(px(8.))
                        .bg(color(palette, ColorToken::StatusStartingBg))
                        .child(system_icon(palette, "icons/info.svg"))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_color(color(palette, ColorToken::AccentDefault))
                                        .child(gui::GUI_SYSTEM_ACTIVITY_SCOPE_TITLE.text(locale)),
                                )
                                .child(muted(
                                    palette,
                                    gui::GUI_SYSTEM_ACTIVITY_SCOPE_COPY.text(locale),
                                )),
                        ),
                );
            }
            let deny = view.clone();
            let deny_id = approval_id.clone();
            let allow = view.clone();
            let allow_id = approval_id.clone();
            let grant = view.clone();
            let grant_id = approval_id.clone();
            let enabled = confirmation.as_ref().is_none_or(|phrase| {
                input
                    .as_ref()
                    .is_some_and(|input| input.read(cx).value().as_ref() == phrase.as_str())
            });
            let expected = confirmation.clone();
            let confirm_input = input.clone();
            let allow_button = Button::new("allow-reviewed-agent-request")
                .small()
                .disabled(!enabled)
                .label(if destroy {
                    gui::GUI_SYSTEM_ACTIVITY_APPROVE_REMOVAL.text(locale)
                } else {
                    gui::GUI_SYSTEM_ACTIVITY_ALLOW_ONCE.text(locale)
                })
                .on_click(move |_, window, cx| {
                    if expected.as_ref().is_some_and(|phrase| {
                        confirm_input
                            .as_ref()
                            .is_none_or(|input| input.read(cx).value().as_ref() != phrase.as_str())
                    }) {
                        return;
                    }
                    window.close_dialog(cx);
                    let _ = allow.update(cx, |view, cx| {
                        view.resolve_approval(allow_id.clone(), Some(false), cx)
                    });
                });
            dialog
                .centered()
                .width(px(680.))
                .overlay_closable(false)
                .title(dialog_title(title.text(locale)))
                .child(content)
                .footer(
                    row(locale)
                        .justify_between()
                        .gap_3()
                        .child(
                            Button::new("deny-reviewed-agent-request")
                                .outline()
                                .small()
                                .label(gui::GUI_DESKTOP_DENY.text(locale))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = deny.update(cx, |view, cx| {
                                        view.resolve_approval(deny_id.clone(), None, cx)
                                    });
                                }),
                        )
                        .child(
                            row(locale)
                                .gap_2()
                                .children(can_grant.then(|| {
                                    Button::new("save-reviewed-agent-grant")
                                        .outline()
                                        .small()
                                        .label(gui::GUI_DESKTOP_ALLOW_FOR_THIS_PROJECT.text(locale))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = grant.update(cx, |view, cx| {
                                                view.resolve_approval(
                                                    grant_id.clone(),
                                                    Some(true),
                                                    cx,
                                                )
                                            });
                                        })
                                }))
                                .child(if destroy {
                                    allow_button.danger()
                                } else {
                                    allow_button.primary()
                                }),
                        ),
                )
        });
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

fn approval_fact(locale: Locale, palette: Palette, key: TextKey, value: &str) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(muted(palette, key.text(locale)))
        .child(mono(value))
}

fn approval_detail(locale: Locale, palette: Palette, key: TextKey, value: &str) -> Div {
    row(locale)
        .gap_2()
        .child(div().w(px(148.)).child(muted(palette, key.text(locale))))
        .child(mono(value))
}

fn permission_pill(locale: Locale, palette: Palette, tier: PermissionTier) -> Div {
    let (key, foreground, background) = match tier {
        PermissionTier::Destroy => (
            gui::GUI_SYSTEM_ACTIVITY_DESTROY,
            ColorToken::StatusError,
            ColorToken::StatusErrorBg,
        ),
        PermissionTier::HostAffecting => (
            gui::GUI_SYSTEM_ACTIVITY_HOST_AFFECTING,
            ColorToken::StatusWarning,
            ColorToken::StatusWarningBg,
        ),
        _ => (
            gui::GUI_SYSTEM_ACTIVITY_OPERATE,
            ColorToken::StatusStarting,
            ColorToken::StatusStartingBg,
        ),
    };
    row(locale)
        .gap_2()
        .rounded_full()
        .px_3()
        .py_1()
        .text_size(px(13.))
        .bg(color(palette, background))
        .text_color(color(palette, foreground))
        .child(
            div()
                .size(px(8.))
                .rounded_full()
                .bg(color(palette, foreground)),
        )
        .child(key.text(locale))
}
