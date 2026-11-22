//! Engine controls, startup diagnostics and the workspace overview.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::path::Path;

use marsa_client::api::{CollectDiagnosticsRequest, EngineState};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, FontWeight, ParentElement, PromptButton, PromptLevel, Styled, Window, div,
    },
    gpui_component::{
        Disableable,
        button::{Button, ButtonVariants},
    },
};

use crate::{
    desktop::{Route, status},
    screens::common::{ScreenContext, color, label, panel, row, technical},
    services::{Command, EngineAction},
};

use super::{Notice, SystemView, disk::bytes, settings::SettingsTab};

impl SystemView {
    pub(crate) fn engine(
        &mut self,
        action: EngineAction,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        if action == EngineAction::Start {
            self.run_engine(action, cx);
            return;
        }
        let title = match action {
            EngineAction::Stop => gui::GUI_DESKTOP_STOP_THE_MACHINE.text(locale),
            _ => gui::GUI_DESKTOP_RESTART_THE_MACHINE.text(locale),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            title,
            Some(
                gui::GUI_DESKTOP_RUNNING_WORKLOADS_WILL_BE_INTERRUPTED_SAVE_YOUR_WORK_FIRST
                    .text(locale),
            ),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_CONTINUE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        self.command = Some(cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.run_engine(action, cx));
            }
        }));
    }

    fn run_engine(&mut self, action: EngineAction, cx: &mut Context<Self>) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        if let Some(services) = &self.services {
            self.action(
                services.dispatch(Command::Engine { action, request_id }),
                |_, _, _| {},
                cx,
            );
        }
    }

    fn collect_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(request_id) = self.mutation_id(cx) else {
            return;
        };
        self.request(
            move |client| async move {
                Ok(client
                    .diagnostics()
                    .collect(CollectDiagnosticsRequest {
                        request_id,
                        include_agent_audit: false,
                    })
                    .await?
                    .into_inner())
            },
            |this, bundle, cx| {
                cx.reveal_path(Path::new(&bundle.bundle_path));
                this.notice = Some(Notice::DiagnosticsSaved);
            },
            cx,
        );
    }

    pub(super) fn machine(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let engine = screen.state.engine();
        let state = engine.map(|engine| engine.state());
        let controls = match state {
            Some(EngineState::Stopped | EngineState::Failed) => vec![EngineAction::Start],
            Some(EngineState::Ready | EngineState::Idle | EngineState::Paused) => {
                vec![EngineAction::Stop, EngineAction::Restart]
            }
            _ => Vec::new(),
        };
        let resources = self.settings.as_ref().map(|(settings, _)| {
            let memory = settings
                .vm
                .memory
                .map(bytes)
                .unwrap_or_else(|| gui::GUI_DESKTOP_AUTOMATIC.text(locale).to_owned());
            let allocation = self
                .disk
                .as_ref()
                .map(|disk| bytes(disk.host_allocated_bytes))
                .unwrap_or_else(|| gui::GUI_DESKTOP_UNAVAILABLE.text(locale).to_owned());
            row(locale)
                .gap_3()
                .child(metric(
                    screen,
                    "Processors",
                    "المعالجات",
                    settings.vm.cpus.to_string(),
                ))
                .child(metric(screen, "Memory", "الذاكرة", memory))
                .child(metric(
                    screen,
                    "Disk allocation",
                    "مساحة القرص المخصّصة",
                    allocation,
                ))
        });
        let boot_message = match state {
            Some(EngineState::Failed) => message(
                screen.locale,
                gui::GUI_DESKTOP_STARTUP_FAILED_REVIEW_THE_STARTUP_DETAILS_AND_COLLECT_DIAGNOST,
            ),
            Some(EngineState::Ready | EngineState::Idle) => message(
                screen.locale,
                gui::GUI_DESKTOP_LAST_BOOT_COMPLETED_THE_ENGINE_IS_READY,
            ),
            _ => message(
                screen.locale,
                gui::GUI_DESKTOP_BOOT_INFORMATION_APPEARS_HERE_AS_THE_MACHINE_STARTS,
            ),
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .gap_3()
                            .child(title_text(gui::GUI_DESKTOP_LINUX_VIRTUAL_MACHINE.text(screen.locale)))
                            .child(row(locale).gap_2().children(controls.into_iter().map(
                                |action| {
                                    let (id, text) = match action {
                                        EngineAction::Start => (
                                            "start-machine",
                                            message(screen.locale, gui::GUI_DESKTOP_START_MACHINE),
                                        ),
                                        EngineAction::Stop => (
                                            "stop-machine",
                                            message(screen.locale, gui::GUI_DESKTOP_STOP_MACHINE),
                                        ),
                                        EngineAction::Restart => (
                                            "restart-machine",
                                            message(screen.locale, gui::GUI_DESKTOP_RESTART_MACHINE),
                                        ),
                                    };
                                    Button::new(id)
                                        .outline()
                                        .disabled(self.busy())
                                        .label(text)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.engine(action, locale, window, cx)
                                        }))
                                },
                            ))),
                    )
                    .child(row(locale).child(status::badge(locale, screen.palette, state)))
                    .child(
                        row(locale)
                            .gap_3()
                            .child(metric(
                                screen,
                                "Backend",
                                "الواجهة الخلفية",
                                engine.map_or("—", |engine| engine.backend.as_str()),
                            ))
                            .child(metric(
                                screen,
                                "Architecture",
                                "البنية",
                                std::env::consts::ARCH,
                            ))
                            .child(metric(screen, "Docker context", "سياق Docker", "marsa")),
                    )
                    .child(message(screen.locale, gui::GUI_DESKTOP_STOPPING_OR_RESTARTING_INTERRUPTS_RUNNING_WORKLOADS)),
            )
            .child(
                panel(screen)
                    .child(title_text(gui::GUI_DESKTOP_RESOURCES.text(screen.locale)))
                    .child(message(screen.locale, gui::GUI_DESKTOP_RESOURCE_CHANGES_TAKE_EFFECT_THE_NEXT_TIME_THE_MACHINE_STARTS))
                    .children(resources)
                    .children(self.restart_required.then(|| {
                        message(screen.locale, gui::GUI_DESKTOP_RESOURCE_CHANGES_ARE_PENDING)
                    }))
                    .child(
                        row(locale).justify_end().child(
                            Button::new("edit-machine-resources")
                                .outline()
                                .disabled(self.settings.is_none())
                                .label(message(screen.locale, gui::GUI_DESKTOP_EDIT_RESOURCES))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.settings_tab = SettingsTab::Resources;
                                    this.show_resource_inputs(window, cx);
                                })),
                        ),
                    )
                    .children(
                        (self.settings_tab == SettingsTab::Resources && self.resource_inputs.open)
                            .then(|| self.resource_editor(screen, cx)),
                    ),
            )
            .child(
                panel(screen)
                    .child(
                        row(locale)
                            .justify_between()
                            .child(title_text(gui::GUI_DESKTOP_BOOT_DIAGNOSTICS.text(screen.locale)))
                            .child(
                                Button::new("startup-details")
                                    .outline()
                                    .label(message(screen.locale, gui::GUI_DESKTOP_VIEW_STARTUP_DETAILS))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.diagnostics_open = !this.diagnostics_open;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(boot_message)
                    .children(
                        engine
                            .and_then(|engine| engine.boot_stage.as_deref())
                            .map(|stage| technical(stage)),
                    )
                    .children(
                        (self.diagnostics_open || state == Some(EngineState::Failed)).then(|| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(
                                    engine
                                        .and_then(|engine| engine.reason.as_deref())
                                        .map(|reason| technical(reason)),
                                )
                                .children(
                                    engine
                                        .and_then(|engine| engine.console_tail.as_deref())
                                        .map(|tail| {
                                            div()
                                                .font_family("Menlo")
                                                .text_sm()
                                                .p_3()
                                                .rounded_md()
                                                .bg(color(screen.palette, ColorToken::BgCode))
                                                .child(technical(tail))
                                        }),
                                )
                                .child(
                                    row(locale).child(
                                        Button::new("collect-diagnostics")
                                            .outline()
                                            .disabled(
                                                self.busy() || screen.state.daemon().is_none(),
                                            )
                                            .label(message(screen.locale, gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.collect_diagnostics(cx)
                                            })),
                                    ),
                                )
                        }),
                    ),
            )
    }
}

pub(super) fn title_text(text: &'static str) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).child(text)
}

pub(super) fn metric(
    screen: &ScreenContext<'_>,
    en: &'static str,
    ar: &'static str,
    value: impl AsRef<str>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .p_2()
        .rounded_md()
        .bg(color(screen.palette, ColorToken::BgSubtle))
        .child(
            div()
                .text_xs()
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(label(screen, en, ar)),
        )
        .child(div().text_sm().child(technical(value.as_ref())))
}
