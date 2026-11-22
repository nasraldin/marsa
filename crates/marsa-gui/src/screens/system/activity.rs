//! Lifecycle history, redacted audit events and owner approvals.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::collections::VecDeque;

use marsa_client::api::{AuditDecision, EngineState, ListAgentAuditRequest};
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement, SharedString, Styled, div},
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants},
    },
};

use crate::{
    desktop::status,
    screens::common::{ScreenContext, color, panel, row, technical},
};

use super::{SystemView, machine::title_text};

impl SystemView {
    pub(super) fn load_audit(&mut self, cx: &Context<Self>) {
        let Some(services) = &self.services else {
            return;
        };
        let decision = self.audit_filter.map(|value| value as i32);
        let cursor = self.audit_cursor.clone();
        let mut pending = services.request(move |client| async move {
            Ok(client
                .agents()
                .list_audit(ListAgentAuditRequest {
                    limit: 100,
                    decision,
                    cursor,
                    ..Default::default()
                })
                .await?
                .into_inner())
        });
        self.route_load = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                this.audit = result.and_then(Result::ok);
                cx.notify();
            });
        }));
    }

    pub(super) fn activity(
        &self,
        screen: &ScreenContext<'_>,
        history: &VecDeque<EngineState>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let snapshot = screen.state.daemon();
        let approvals = snapshot.and_then(|snapshot| snapshot.approvals.as_ref());
        let grants = snapshot.and_then(|snapshot| snapshot.grants.as_ref());
        let disabled = self.busy() || snapshot.is_none();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_PENDING_AGENT_APPROVALS.text(screen.locale),
                    ))
                    .children(
                        approvals
                            .is_some_and(|items| items.approvals.is_empty())
                            .then(|| {
                                message(screen.locale, gui::GUI_DESKTOP_NO_PENDING_AGENT_APPROVALS)
                            }),
                    )
                    .children(
                        approvals
                            .into_iter()
                            .flat_map(|items| &items.approvals)
                            .map(|approval| self.approval(screen, approval, cx)),
                    ),
            )
            .children(
                snapshot
                    .and_then(|snapshot| snapshot.bind_approvals.as_ref())
                    .filter(|items| !items.approvals.is_empty())
                    .map(|_| self.sharing(screen, cx)),
            )
            .child(
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_PROJECT_GRANTS.text(screen.locale),
                    ))
                    .children(
                        grants
                            .is_some_and(|items| items.grants.is_empty())
                            .then(|| {
                                message(screen.locale, gui::GUI_DESKTOP_NO_ACTIVE_PROJECT_GRANTS)
                            }),
                    )
                    .children(
                        grants
                            .into_iter()
                            .flat_map(|items| &items.grants)
                            .map(|grant| {
                                let id = grant.id.clone();
                                row(locale)
                                    .justify_between()
                                    .gap_2()
                                    .border_t_1()
                                    .py_2()
                                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(technical(&grant.project_path))
                                            .child(technical(grant.operation().as_str_name())),
                                    )
                                    .child(
                                        Button::new(format!("revoke-grant-{id}"))
                                            .outline()
                                            .disabled(disabled)
                                            .label(message(screen.locale, gui::GUI_DESKTOP_REVOKE))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.confirm_revoke(id.clone(), locale, window, cx)
                                            })),
                                    )
                            }),
                    ),
            )
            .child(
                panel(screen)
                    .child(title_text(
                        gui::GUI_DESKTOP_ENGINE_EVENTS.text(screen.locale),
                    ))
                    .children(history.is_empty().then(|| {
                        message(screen.locale, gui::GUI_DESKTOP_NO_EVENTS_IN_THIS_SESSION)
                    }))
                    .children(history.iter().rev().map(|event| {
                        row(locale).child(status::badge(locale, screen.palette, Some(*event)))
                    })),
            )
            .child(
                panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .child(title_text(
                                gui::GUI_DESKTOP_AGENT_AUDIT_LOG.text(screen.locale),
                            ))
                            .child(
                                Button::new("refresh-audit")
                                    .outline()
                                    .disabled(snapshot.is_none())
                                    .label(message(screen.locale, gui::GUI_DESKTOP_REFRESH))
                                    .on_click(cx.listener(|this, _, _, cx| this.load_audit(cx))),
                            ),
                    )
                    .child(
                        row(locale).gap_2().flex_wrap().children(
                            [
                                None,
                                Some(AuditDecision::Allowed),
                                Some(AuditDecision::Approved),
                                Some(AuditDecision::Denied),
                                Some(AuditDecision::Refused),
                                Some(AuditDecision::Expired),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, filter)| {
                                let text = filter.map_or_else(
                                    || message(screen.locale, gui::GUI_DESKTOP_ALL_DECISIONS),
                                    |filter| decision(screen, filter),
                                );
                                Button::new(("audit-filter", index))
                                    .ghost()
                                    .selected(self.audit_filter == filter)
                                    .label(text)
                                    .disabled(disabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.audit_filter = filter;
                                        this.audit_cursor = None;
                                        this.load_audit(cx);
                                        cx.notify();
                                    }))
                            }),
                        ),
                    )
                    .children(
                        self.audit
                            .as_ref()
                            .and_then(|audit| audit.next_cursor.clone())
                            .map(|cursor| {
                                Button::new("older-audit-entries")
                                    .outline()
                                    .label(message(screen.locale, gui::GUI_DESKTOP_OLDER_ENTRIES))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.audit_cursor = Some(cursor.clone());
                                        this.load_audit(cx);
                                    }))
                            }),
                    )
                    .children(self.audit.as_ref().map_or_else(
                        || {
                            Some(message(
                                screen.locale,
                                gui::GUI_DESKTOP_AUDIT_LOG_UNAVAILABLE_TRY_REFRESHING,
                            ))
                        },
                        |audit| {
                            audit
                                .records
                                .is_empty()
                                .then(|| message(screen.locale, gui::GUI_DESKTOP_NO_AUDIT_EVENTS))
                        },
                    ))
                    .children(
                        self.audit
                            .iter()
                            .flat_map(|audit| &audit.records)
                            .map(|record| {
                                row(locale)
                                    .justify_between()
                                    .gap_2()
                                    .border_t_1()
                                    .py_2()
                                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                                    .child(technical(record.operation().as_str_name()))
                                    .child(technical(record.client_ref.as_deref().unwrap_or("—")))
                                    .child(decision(screen, record.decision()))
                            }),
                    ),
            )
    }
}

fn decision(screen: &ScreenContext<'_>, value: AuditDecision) -> SharedString {
    match value {
        AuditDecision::Refused => message(screen.locale, gui::GUI_DESKTOP_REFUSED),
        AuditDecision::Allowed => message(screen.locale, gui::GUI_DESKTOP_ALLOWED),
        AuditDecision::Approved => message(screen.locale, gui::GUI_DESKTOP_APPROVED),
        AuditDecision::Denied => message(screen.locale, gui::GUI_DESKTOP_DENIED),
        AuditDecision::Expired => message(screen.locale, gui::GUI_DESKTOP_EXPIRED),
    }
}
