//! Lifecycle history, redacted audit events and owner approvals.

use crate::screens::common::message;
use marsa_ui::{gui_keys as gui, tokens::typography::ui_size};
use std::collections::VecDeque;

use marsa_client::api::{AgentAuditRecord, AuditDecision, EngineState, ListAgentAuditRequest};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, SharedString, Styled, div, px},
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants},
    },
    preferences::TimeFormat,
};

use crate::{
    desktop::status,
    screens::common::{ScreenContext, color, day_periods, panel, row},
};

use super::{
    SystemView,
    machine::{mono, muted, system_icon, title_text},
};

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
        let palette = screen.palette;
        let pending_count = approvals.map_or(0, |items| items.approvals.len());
        let audit_count = self.audit.as_ref().map_or(0, |items| items.records.len());
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(screen)
                    .pt_3()
                    .gap_3()
                    .min_h(px(260.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(title_text(
                                gui::GUI_SYSTEM_ACTIVITY_AGENT_REQUESTS.text(locale),
                            ))
                            .child(muted(
                                palette,
                                gui::GUI_SYSTEM_ACTIVITY_LOCAL_ENGINE.text(locale),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .children((pending_count == 0 && audit_count == 0).then(|| {
                                muted(
                                    palette,
                                    if snapshot.is_some() {
                                        gui::GUI_DESKTOP_NO_PENDING_AGENT_APPROVALS.text(locale)
                                    } else {
                                        gui::GUI_SYSTEM_LOCAL_DAEMON_UNAVAILABLE.text(locale)
                                    },
                                )
                            }))
                            .children(
                                approvals
                                    .into_iter()
                                    .flat_map(|items| &items.approvals)
                                    .map(|approval| self.approval(screen, approval, cx)),
                            )
                            .children(
                                self.audit
                                    .iter()
                                    .flat_map(|audit| audit.records.iter().take(8))
                                    .map(|record| audit_row(screen, record)),
                            ),
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
                    .pt_3()
                    .child(title_text(gui::GUI_DESKTOP_ENGINE_EVENTS.text(locale)))
                    .children(history.is_empty().then(|| {
                        muted(
                            palette,
                            gui::GUI_DESKTOP_NO_EVENTS_IN_THIS_SESSION.text(locale),
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .children(history.iter().rev().map(|event| {
                                let icon = match event {
                                    EngineState::Ready | EngineState::Idle => "icons/play.svg",
                                    EngineState::Stopped | EngineState::Stopping => {
                                        "icons/square.svg"
                                    }
                                    EngineState::Failed => "icons/triangle-alert.svg",
                                    _ => "icons/activity.svg",
                                };
                                row(locale)
                                    .min_h(px(52.))
                                    .gap_3()
                                    .border_t_1()
                                    .border_color(color(palette, ColorToken::BorderSubtle))
                                    .child(system_icon(palette, icon))
                                    .child(status::text(locale, Some(*event)))
                            })),
                    ),
            )
            .child(
                panel(screen)
                    .pt_3()
                    .child(title_text(gui::GUI_DESKTOP_PROJECT_GRANTS.text(locale)))
                    .children(
                        grants
                            .is_some_and(|items| items.grants.is_empty())
                            .then(|| {
                                muted(
                                    palette,
                                    gui::GUI_DESKTOP_NO_ACTIVE_PROJECT_GRANTS.text(locale),
                                )
                            }),
                    )
                    .child(
                        div().flex().flex_col().children(
                            grants
                                .into_iter()
                                .flat_map(|items| &items.grants)
                                .map(|grant| {
                                    let id = grant.id.clone();
                                    row(locale)
                                        .min_h(px(52.))
                                        .justify_between()
                                        .gap_3()
                                        .border_t_1()
                                        .border_color(color(palette, ColorToken::BorderSubtle))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(mono(&grant.project_path))
                                                .child(
                                                    mono(grant.operation().as_str_name())
                                                        .text_color(color(
                                                            palette,
                                                            ColorToken::TextMuted,
                                                        )),
                                                ),
                                        )
                                        .child(
                                            Button::new(format!("revoke-grant-{id}"))
                                                .outline()
                                                .disabled(disabled)
                                                .label(gui::GUI_DESKTOP_REVOKE.text(locale))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.confirm_revoke(
                                                            id.clone(),
                                                            locale,
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        )
                                }),
                        ),
                    ),
            )
            .child(
                panel(screen)
                    .pt_3()
                    .child(
                        row(locale)
                            .justify_between()
                            .child(title_text(gui::GUI_DESKTOP_AGENT_AUDIT_LOG.text(locale)))
                            .child(
                                Button::new("refresh-audit")
                                    .outline()
                                    .disabled(snapshot.is_none())
                                    .label(gui::GUI_DESKTOP_REFRESH.text(locale))
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
                                    || message(locale, gui::GUI_DESKTOP_ALL_DECISIONS),
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
                    .children(self.audit.as_ref().map_or_else(
                        || {
                            Some(muted(
                                palette,
                                gui::GUI_DESKTOP_AUDIT_LOG_UNAVAILABLE_TRY_REFRESHING.text(locale),
                            ))
                        },
                        |audit| {
                            audit.records.is_empty().then(|| {
                                muted(palette, gui::GUI_DESKTOP_NO_AUDIT_EVENTS.text(locale))
                            })
                        },
                    ))
                    .child(
                        div().flex().flex_col().children(
                            self.audit
                                .iter()
                                .flat_map(|audit| &audit.records)
                                .map(|record| audit_row(screen, record)),
                        ),
                    )
                    .children(
                        self.audit
                            .as_ref()
                            .and_then(|audit| audit.next_cursor.clone())
                            .map(|cursor| {
                                row(locale).justify_end().child(
                                    Button::new("older-audit-entries")
                                        .outline()
                                        .label(gui::GUI_DESKTOP_OLDER_ENTRIES.text(locale))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.audit_cursor = Some(cursor.clone());
                                            this.load_audit(cx);
                                        })),
                                )
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

fn audit_row(screen: &ScreenContext<'_>, record: &AgentAuditRecord) -> Div {
    let locale = screen.locale;
    let palette = screen.palette;
    let result = record.decision();
    let tone = match result {
        AuditDecision::Approved | AuditDecision::Allowed => ColorToken::StatusRunning,
        AuditDecision::Denied | AuditDecision::Refused => ColorToken::StatusError,
        AuditDecision::Expired => ColorToken::TextMuted,
    };
    row(locale)
        .min_h(px(53.))
        .gap_3()
        .border_t_1()
        .border_color(color(palette, ColorToken::BorderSubtle))
        .child(
            div()
                .w(px(112.))
                .text_size(ui_size(13.))
                .text_color(color(palette, tone))
                .child(decision(screen, result)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(mono(record.operation().as_str_name()))
                .children(
                    record.client_ref.as_deref().map(|client| {
                        mono(client).text_color(color(palette, ColorToken::TextMuted))
                    }),
                ),
        )
        .children((record.received_at_unix_millis > 0).then(|| {
            mono(&utc_time(
                locale,
                screen.display.time_format,
                record.received_at_unix_millis,
            ))
            .text_color(color(palette, ColorToken::TextMuted))
        }))
}

pub(super) fn utc_time(locale: Locale, format: TimeFormat, timestamp: i64) -> String {
    let seconds = timestamp.div_euclid(1000).rem_euclid(86400);
    let hour = seconds / 3600;
    let minute = seconds / 60 % 60;
    let second = seconds % 60;
    match format {
        TimeFormat::TwentyFourHour => format!("{hour:02}:{minute:02}:{second:02} UTC"),
        TimeFormat::TwelveHour => {
            let (am, pm) = day_periods(locale);
            let period = if hour < 12 { am } else { pm };
            let hour_12 = (hour + 11) % 12 + 1;
            format!("{hour_12:02}:{minute:02}:{second:02} {period} UTC")
        }
    }
}
