//! Engine controls, resource preferences and startup diagnostics.

use marsa_ui::gpui_component::scroll::ScrollableElement as _;
use marsa_ui::tokens::typography::ui_size;

use std::{cell::Cell, path::Path, rc::Rc};

use marsa_client::api::{CollectDiagnosticsRequest, EngineState, EngineStatus};
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement, Styled, Window,
        div, px,
    },
    gpui_component::{
        Disableable, Icon, IconName, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
    },
    gui_keys as gui,
};

use crate::{
    desktop::status,
    screens::common::{
        ScreenContext, align, color, dialog_actions, dialog_frame, dialog_heading, message, panel,
        row, technical,
    },
    screens::external_inventory::ExternalWorkspace,
    services::{Command, EngineAction},
};

use super::{Notice, SystemView, disk::bytes};

const GIB: u64 = 1024 * 1024 * 1024;

impl SystemView {
    pub(crate) fn engine(
        &mut self,
        action: EngineAction,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy() || window.has_active_dialog(cx) {
            return;
        }
        if action == EngineAction::Start {
            self.run_engine(action, cx);
            return;
        }
        let stopping = action == EngineAction::Stop;
        let title = if stopping {
            gui::GUI_SYSTEM_MACHINE_CONFIRM_STOP
        } else {
            gui::GUI_SYSTEM_MACHINE_CONFIRM_RESTART
        };
        let explanation = if stopping {
            gui::GUI_SYSTEM_MACHINE_STOP_EXPLANATION
        } else {
            gui::GUI_SYSTEM_MACHINE_RESTART_EXPLANATION
        };
        let submit = if stopping {
            gui::GUI_DESKTOP_STOP_MACHINE
        } else {
            gui::GUI_DESKTOP_RESTART_MACHINE
        };
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            dialog_frame(dialog, 570.)
                .overlay_closable(false)
                .title(dialog_heading(locale, palette, title.text(locale)))
                .child(
                    align(div().flex().flex_col().gap_2().py_2(), locale)
                        .child(explanation.text(locale))
                        .child(muted(
                            palette,
                            gui::GUI_SYSTEM_MACHINE_VOLUMES_STAY.text(locale),
                        )),
                )
                .footer(
                    dialog_actions(locale, palette)
                        .child(cancel_button("cancel-machine-action", locale))
                        .child(
                            Button::new("confirm-machine-action")
                                .danger()
                                .small()
                                .label(submit.text(locale))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = view.update(cx, |view, cx| view.run_engine(action, cx));
                                }),
                        ),
                )
        });
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

    pub(crate) fn collect_diagnostics(&mut self, cx: &mut Context<Self>) {
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
        let palette = screen.palette;
        let engine = screen.state.engine();
        let state = engine.map(EngineStatus::state);
        if state == Some(EngineState::Failed) {
            return self.failed_engine(screen, cx);
        }
        let controls: &[EngineAction] = match state {
            Some(EngineState::Stopped | EngineState::Failed) => &[EngineAction::Start],
            Some(EngineState::Ready | EngineState::Idle | EngineState::Paused) => {
                &[EngineAction::Stop, EngineAction::Restart]
            }
            _ => &[],
        };
        let running = matches!(
            state,
            Some(EngineState::Ready | EngineState::Idle | EngineState::Paused)
        );
        let backend = engine.map_or("—", |engine| match engine.backend.as_str() {
            "vz" => "Virtualization.framework",
            "krun" => "libkrun",
            name => name,
        });
        let resources = self.settings.as_ref().map(|(settings, _)| {
            machine_resource_values(
                screen,
                format!("{} vCPU", settings.vm.cpus),
                settings
                    .vm
                    .memory
                    .map(|value| bytes(locale, value))
                    .unwrap_or_else(|| gui::GUI_DESKTOP_AUTOMATIC.text(locale).to_owned()),
                self.disk
                    .as_ref()
                    .map(|disk| bytes(locale, disk.host_allocated_bytes))
                    .unwrap_or_else(|| gui::GUI_DESKTOP_UNAVAILABLE.text(locale).to_owned()),
            )
        });
        let actions = row(locale)
            .gap_2()
            .flex_wrap()
            .children(controls.iter().copied().map(|action| {
                let (id, key, glyph) = match action {
                    EngineAction::Start => (
                        "start-machine",
                        gui::GUI_DESKTOP_START_MACHINE,
                        IconName::Play,
                    ),
                    EngineAction::Stop => (
                        "stop-machine",
                        gui::GUI_DESKTOP_STOP_MACHINE,
                        IconName::Square,
                    ),
                    EngineAction::Restart => (
                        "restart-machine",
                        gui::GUI_DESKTOP_RESTART_MACHINE,
                        IconName::RefreshCw,
                    ),
                };
                let button = Button::new(id)
                    .icon(glyph)
                    .disabled(self.busy())
                    .label(key.text(locale))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.engine(action, locale, palette, window, cx)
                    }));
                if matches!(action, EngineAction::Start) {
                    button.primary()
                } else {
                    button.outline()
                }
            }));
        let detail = engine.cloned();
        let resource_action = (!self.restart_required).then(|| {
            Button::new("edit-machine-resources")
                .outline()
                .disabled(self.settings.is_none() || self.busy())
                .label(gui::GUI_DESKTOP_EDIT_RESOURCES.text(locale))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.edit_machine_resources(locale, palette, window, cx)
                }))
        });
        let resource_body = div()
            .flex()
            .flex_col()
            .gap_3()
            .children(resources)
            .children(self.restart_required.then(|| {
                div()
                    .text_sm()
                    .text_color(color(palette, ColorToken::StatusWarning))
                    .child(gui::GUI_DESKTOP_RESOURCE_CHANGES_ARE_PENDING.text(locale))
            }));
        let diagnostics = div().flex().flex_col().gap_1().children(
            matches!(
                state,
                Some(EngineState::Starting | EngineState::Booting | EngineState::Failed)
            )
            .then(|| engine.and_then(|engine| engine.boot_stage.as_deref()))
            .flatten()
            .map(mono),
        );
        let startup = Button::new("startup-details")
            .outline()
            .label(gui::GUI_DESKTOP_VIEW_STARTUP_DETAILS.text(locale))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.startup_details(detail.clone(), locale, palette, window, cx)
            }));
        machine_layout(
            machine_identity(
                screen,
                gui::GUI_DESKTOP_LINUX_VIRTUAL_MACHINE.text(locale).into(),
                status::compact_badge(locale, palette, state),
                [
                    (gui::GUI_DESKTOP_BACKEND, backend),
                    (gui::GUI_SYSTEM_MACHINE_ARCHITECTURE, host_architecture()),
                    (
                        gui::GUI_DESKTOP_DOCKER_CONTEXT,
                        if engine.is_some() { "marsa" } else { "—" },
                    ),
                ],
                actions,
                running.then(|| {
                    muted(
                        palette,
                        gui::GUI_DESKTOP_RUNNING_WORKLOADS_WILL_BE_INTERRUPTED_SAVE_YOUR_WORK_FIRST
                            .text(locale),
                    )
                }),
            ),
            machine_section(
                screen,
                gui::GUI_DESKTOP_RESOURCES,
                gui::GUI_SYSTEM_MACHINE_RESOURCE_NOTICE.text(locale),
                resource_action,
                resource_body,
            ),
            machine_section(
                screen,
                gui::GUI_DESKTOP_BOOT_DIAGNOSTICS,
                startup_copy(state).text(locale),
                Some(startup),
                diagnostics,
            ),
        )
    }

    pub(crate) fn external_machine(
        &self,
        name: &str,
        workspace: Option<&ExternalWorkspace>,
        connection: &'static str,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let lima = self.has_lima_machine_target();
        let settings = self.lima_machine_settings();
        let state = self.lima_machine_state();
        let unavailable = gui::GUI_DESKTOP_UNAVAILABLE.text(locale);
        let status = if lima && state.is_some() {
            status::compact_badge(locale, palette, state)
        } else {
            status::badge_with_text(
                locale,
                palette,
                workspace.map(|workspace| {
                    if workspace.reachable {
                        EngineState::Ready
                    } else {
                        EngineState::Failed
                    }
                }),
                connection,
            )
        };
        let refresh = Button::new("refresh-machine-connection")
            .outline()
            .label(gui::GUI_DESKTOP_REFRESH.text(locale))
            .on_click(cx.listener(|this, _, window, cx| this.refresh_lima_selection(window, cx)));
        let resources = settings.map_or_else(
            || {
                machine_resource_values(
                    screen,
                    unavailable.into(),
                    unavailable.into(),
                    unavailable.into(),
                )
            },
            |settings| {
                machine_resource_values(
                    screen,
                    format!("{} vCPU", settings.cpus),
                    bytes(locale, settings.memory_bytes),
                    bytes(locale, settings.disk_bytes),
                )
            },
        );
        let resource_notice = if let Some(settings) = settings {
            if settings.stopped {
                gui::GUI_ENVIRONMENT_LIMA_STOPPED
            } else if settings.running {
                gui::GUI_ENVIRONMENT_LIMA_RUNNING
            } else {
                gui::GUI_ENVIRONMENT_LIMA_UNKNOWN_STATE
            }
        } else if lima {
            gui::GUI_SYSTEM_MACHINE_LIMA_RESOURCES_UNAVAILABLE
        } else {
            gui::GUI_SYSTEM_MACHINE_PROVIDER_MANAGED
        };
        let version = workspace.and_then(|workspace| workspace.verified_engine.as_ref());
        let address = workspace.map(|workspace| workspace.endpoint.address());
        let network = settings
            .and_then(|settings| settings.host_loopback)
            .map(|allowed| {
                muted(
                    palette,
                    if allowed {
                        gui::GUI_SYSTEM_MACHINE_LOOPBACK_ALLOWED
                    } else {
                        gui::GUI_SYSTEM_MACHINE_LOOPBACK_DENIED
                    }
                    .text(locale),
                )
            });
        let diagnostics = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                row(locale)
                    .gap_3()
                    .flex_wrap()
                    .child(machine_fact_value(
                        screen,
                        gui::GUI_SYSTEM_MACHINE_DOCKER_CONNECTION,
                        div()
                            .text_size(ui_size(12.))
                            .line_height(ui_size(18.))
                            .child(connection),
                    ))
                    .children(address.as_deref().map(|value| {
                        machine_fact(screen, gui::GUI_SYSTEM_MACHINE_DOCKER_ENDPOINT, value)
                    }))
                    .children(version.map(|engine| {
                        machine_fact(
                            screen,
                            gui::GUI_SYSTEM_MACHINE_DOCKER_VERSION,
                            &engine.version,
                        )
                    })),
            )
            .children(self.lima_machine_feedback(screen));
        machine_layout(
            machine_identity(
                screen,
                technical(name),
                status,
                [
                    (
                        gui::GUI_DESKTOP_BACKEND,
                        settings.map_or(if lima { "Lima" } else { "—" }, |s| s.backend),
                    ),
                    (
                        gui::GUI_SYSTEM_MACHINE_ARCHITECTURE,
                        settings.and_then(|s| s.architecture).unwrap_or("—"),
                    ),
                    (gui::GUI_DESKTOP_DOCKER_CONTEXT, name),
                ],
                self.lima_machine_actions(locale, cx),
                None,
            ),
            machine_section(
                screen,
                gui::GUI_DESKTOP_RESOURCES,
                resource_notice.text(locale),
                lima.then(|| self.lima_machine_edit_button(locale, cx)),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(resources)
                    .children(network)
                    .children(self.lima_machine_editor(screen, cx)),
            ),
            machine_section(
                screen,
                gui::GUI_DESKTOP_BOOT_DIAGNOSTICS,
                gui::GUI_SYSTEM_MACHINE_EXTERNAL_DIAGNOSTICS.text(locale),
                Some(refresh),
                diagnostics,
            ),
        )
    }

    pub(crate) fn failed_engine(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let engine = screen.state.engine();
        let current = engine.and_then(|engine| engine.boot_stage.as_deref());
        let actual = match current {
            Some("prepare" | "create" | "start") => (Some(0), 0),
            Some("handshake") => (Some(1), 1),
            Some("data_disk" | "network") => (None, 2),
            Some("engine" | "running") => (Some(2), 2),
            _ => (None, 0),
        };
        let stages = [
            gui::GUI_SYSTEM_MACHINE_STAGE_VM,
            gui::GUI_SYSTEM_MACHINE_STAGE_AGENT,
            gui::GUI_SYSTEM_MACHINE_STAGE_ENGINE,
        ];
        let caption = actual.0.map(|index| stages[index].text(locale)).or(current);
        let reason = engine
            .and_then(|engine| engine.reason.as_deref())
            .filter(|reason| !reason.is_empty());
        let console = engine
            .and_then(|engine| engine.console_tail.as_deref())
            .filter(|tail| !tail.is_empty());
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                row(locale)
                    .gap_3()
                    .items_start()
                    .p_3()
                    .rounded(px(8.))
                    .bg(color(palette, ColorToken::StatusErrorBg))
                    .child(system_icon(palette, "icons/triangle-alert.svg"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                row(locale)
                                    .gap_2()
                                    .text_size(ui_size(14.))
                                    .text_color(color(palette, ColorToken::StatusError))
                                    .child(gui::GUI_SYSTEM_MACHINE_FAILED_STAGE.text(locale))
                                    .children(caption.map(|stage| {
                                        row(locale).gap_2().child("·").child(stage.to_owned())
                                    })),
                            )
                            .children(reason.map(mono)),
                    ),
            )
            .child(
                row(locale)
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("failed-machine-diagnostics")
                            .outline()
                            .icon(Icon::default().path("icons/download.svg"))
                            .label(gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS.text(locale))
                            .disabled(self.busy() || self.services.is_none())
                            .on_click(cx.listener(|view, _, _, cx| view.collect_diagnostics(cx))),
                    )
                    .child(
                        Button::new("retry-failed-machine")
                            .primary()
                            .icon(Icon::default().path("icons/refresh-cw.svg"))
                            .label(gui::GUI_DESKTOP_RETRY.text(locale))
                            .disabled(self.busy() || self.services.is_none())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.run_engine(EngineAction::Start, cx)
                            })),
                    ),
            )
            .child(
                panel(screen)
                    .rounded(px(8.))
                    .child(title_text(gui::GUI_SYSTEM_MACHINE_BOOT_STAGES.text(locale)))
                    .child(row(locale).gap_3().flex_wrap().children(
                        stages.into_iter().enumerate().map(|(index, label)| {
                            let (icon, tone) = if actual.0 == Some(index) {
                                ("icons/triangle-alert.svg", ColorToken::StatusError)
                            } else if index < actual.1 {
                                ("icons/check.svg", ColorToken::StatusRunning)
                            } else {
                                ("icons/clock-3.svg", ColorToken::TextMuted)
                            };
                            row(locale)
                                .flex_1()
                                .min_w(px(128.))
                                .gap_2()
                                .text_size(ui_size(13.))
                                .text_color(color(palette, tone))
                                .child(system_icon(palette, icon))
                                .child(label.text(locale))
                        }),
                    )),
            )
            .child(
                panel(screen)
                    .rounded(px(8.))
                    .bg(color(palette, ColorToken::BgCode))
                    .child(title_text(
                        gui::GUI_SYSTEM_MACHINE_RECENT_BOOT_LOGS.text(locale),
                    ))
                    .child(
                        div()
                            .id("failed-machine-boot-logs")
                            .max_h(px(316.))
                            .overflow_y_scrollbar()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .children(console.into_iter().flat_map(str::lines).map(mono))
                            .children(console.is_none().then(|| {
                                muted(
                                    palette,
                                    gui::GUI_SYSTEM_MACHINE_NO_STARTUP_DETAILS.text(locale),
                                )
                            })),
                    ),
            )
    }

    fn edit_machine_resources(
        &mut self,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((settings, _)) = self.settings.as_ref() else {
            return;
        };
        if self.busy() || window.has_active_dialog(cx) {
            return;
        }
        let cpus = Rc::new(Cell::new(settings.vm.cpus));
        let memory = Rc::new(Cell::new(settings.vm.memory));
        let mut cpu_choices = vec![1, 2, 4, 6, 8, 12, 16, settings.vm.cpus];
        cpu_choices.sort_unstable();
        cpu_choices.dedup();
        let mut memory_choices = vec![
            None,
            Some(2 * GIB),
            Some(4 * GIB),
            Some(8 * GIB),
            Some(12 * GIB),
            Some(16 * GIB),
            settings.vm.memory,
        ];
        memory_choices.sort_unstable();
        memory_choices.dedup();
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let cpu_for_save = cpus.clone();
            let memory_for_save = memory.clone();
            let processors = gui::GUI_SYSTEM_MACHINE_PROCESSORS.text(locale);
            let ram = gui::GUI_SYSTEM_MACHINE_MEMORY.text(locale);
            let fields = align(div().flex().flex_col().gap_4(), locale)
                .child(muted(
                    palette,
                    gui::GUI_SYSTEM_MACHINE_AFTER_NEXT_RESTART.text(locale),
                ))
                .child(
                    row(locale)
                        .items_start()
                        .flex_wrap()
                        .gap_3()
                        .child(resource_field(
                            processors,
                            resource_menu(
                                "machine-cpu-count",
                                processors,
                                locale,
                                cpus.clone(),
                                &cpu_choices,
                                |cpu| format!("{cpu} vCPU"),
                            ),
                        ))
                        .child(resource_field(
                            ram,
                            resource_menu(
                                "machine-memory-size",
                                ram,
                                locale,
                                memory.clone(),
                                &memory_choices,
                                move |memory| {
                                    memory.map(|value| bytes(locale, value)).unwrap_or_else(|| {
                                        gui::GUI_DESKTOP_AUTOMATIC.text(locale).to_owned()
                                    })
                                },
                            ),
                        )),
                );
            dialog_frame(dialog, 576.)
                .overlay_closable(false)
                .title(dialog_heading(
                    locale,
                    palette,
                    gui::GUI_DESKTOP_EDIT_RESOURCES.text(locale),
                ))
                .child(fields)
                .footer(
                    dialog_actions(locale, palette)
                        .child(cancel_button("cancel-machine-resources", locale))
                        .child(
                            Button::new("confirm-machine-resources")
                                .primary()
                                .small()
                                .label(gui::GUI_SYSTEM_MACHINE_SAVE_CHANGES.text(locale))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let cpu = cpu_for_save.get();
                                    let memory = memory_for_save.get();
                                    let _ = view.update(cx, |view, cx| {
                                        view.save_settings(
                                            |settings| {
                                                settings.vm.cpus = cpu;
                                                settings.vm.memory = memory;
                                            },
                                            cx,
                                        )
                                    });
                                }),
                        ),
                )
        });
    }

    fn startup_details(
        &self,
        engine: Option<EngineStatus>,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let context = startup_copy(engine.as_ref().map(EngineStatus::state));
        let enabled = self.services.is_some() && engine.is_some();
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let details = engine
                .as_ref()
                .into_iter()
                .flat_map(|engine| {
                    [
                        engine.boot_stage.as_deref(),
                        engine.reason.as_deref(),
                        engine.console_tail.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|line| !line.is_empty())
                })
                .collect::<Vec<_>>();
            dialog_frame(dialog, 660.)
                .title(dialog_heading(
                    locale,
                    palette,
                    gui::GUI_DESKTOP_BOOT_DIAGNOSTICS.text(locale),
                ))
                .child(
                    align(div().flex().flex_col().gap_5(), locale)
                        .child(muted(palette, context.text(locale)))
                        .child(
                            div()
                                .min_h(px(88.))
                                .rounded(px(12.))
                                .border_1()
                                .border_color(color(palette, ColorToken::BorderSubtle))
                                .bg(color(palette, ColorToken::BgCode))
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .children(details.is_empty().then(|| {
                                    muted(
                                        palette,
                                        gui::GUI_SYSTEM_MACHINE_NO_STARTUP_DETAILS.text(locale),
                                    )
                                }))
                                .children(details.into_iter().map(mono)),
                        ),
                )
                .footer(
                    dialog_actions(locale, palette)
                        .child(
                            Button::new("close-startup-details")
                                .outline()
                                .small()
                                .label(gui::GUI_DESKTOP_CLOSE.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("collect-diagnostics")
                                .outline()
                                .small()
                                .icon(Icon::default().path("icons/download.svg"))
                                .label(message(locale, gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS))
                                .disabled(!enabled)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        view.update(cx, |view, cx| view.collect_diagnostics(cx));
                                }),
                        ),
                )
        });
    }
}

fn host_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        arch => arch,
    }
}

fn startup_copy(state: Option<EngineState>) -> TextKey {
    match state {
        Some(EngineState::Failed) => {
            gui::GUI_DESKTOP_STARTUP_FAILED_REVIEW_THE_STARTUP_DETAILS_AND_COLLECT_DIAGNOST
        }
        Some(EngineState::Ready | EngineState::Idle) => {
            gui::GUI_DESKTOP_LAST_BOOT_COMPLETED_THE_ENGINE_IS_READY
        }
        Some(EngineState::Stopped) => gui::GUI_SYSTEM_MACHINE_STOPPED_HINT,
        _ => gui::GUI_DESKTOP_BOOT_INFORMATION_APPEARS_HERE_AS_THE_MACHINE_STARTS,
    }
}

pub(super) fn title_text(text: impl Into<marsa_ui::gpui::SharedString>) -> Div {
    div()
        .text_size(ui_size(16.))
        .line_height(ui_size(24.))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

pub(super) fn mono(value: &str) -> Div {
    div()
        .min_w_0()
        .font_family("IBM Plex Mono")
        .text_size(ui_size(12.))
        .line_height(ui_size(18.))
        .child(technical(value))
}

pub(super) fn muted(palette: Palette, value: &'static str) -> Div {
    div()
        .text_size(ui_size(12.))
        .line_height(ui_size(18.))
        .text_color(color(palette, ColorToken::TextMuted))
        .child(value)
}

pub(super) fn system_icon(palette: Palette, path: &'static str) -> Icon {
    Icon::default()
        .path(path)
        .with_size(px(20.))
        .text_color(color(palette, ColorToken::TextMuted))
}

fn machine_layout(identity: Div, resources: Div, diagnostics: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(identity)
        .child(resources)
        .child(diagnostics)
}

fn machine_identity(
    screen: &ScreenContext<'_>,
    name: marsa_ui::gpui::SharedString,
    badge: Div,
    facts: [(TextKey, &str); 3],
    actions: Div,
    notice: Option<Div>,
) -> Div {
    panel(screen)
        .child(
            row(screen.locale)
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    row(screen.locale)
                        .gap_2()
                        .flex_wrap()
                        .child(system_icon(screen.palette, "icons/server.svg"))
                        .child(title_text(name).min_w_0().truncate())
                        .child(badge),
                )
                .child(actions),
        )
        .children(notice)
        .child(
            row(screen.locale)
                .flex_wrap()
                .gap_3()
                .border_t_1()
                .pt_3()
                .border_color(color(screen.palette, ColorToken::BorderSubtle))
                .children(
                    facts
                        .into_iter()
                        .map(|(label, value)| machine_fact(screen, label, value)),
                ),
        )
}

fn machine_section(
    screen: &ScreenContext<'_>,
    heading: TextKey,
    description: &'static str,
    action: Option<Button>,
    body: Div,
) -> Div {
    panel(screen)
        .child(
            row(screen.locale)
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(title_text(heading.text(screen.locale)))
                        .child(muted(screen.palette, description)),
                )
                .children(action),
        )
        .child(body)
}

fn machine_resource_values(
    screen: &ScreenContext<'_>,
    cpu: String,
    memory: String,
    disk: String,
) -> Div {
    row(screen.locale)
        .flex_wrap()
        .gap_3()
        .child(resource(
            screen,
            "icons/cpu.svg",
            gui::GUI_SYSTEM_MACHINE_PROCESSORS,
            cpu,
        ))
        .child(resource(
            screen,
            "icons/memory-stick.svg",
            gui::GUI_SYSTEM_MACHINE_MEMORY,
            memory,
        ))
        .child(resource(
            screen,
            "icons/hard-drive.svg",
            gui::GUI_SYSTEM_MACHINE_DISK_ALLOCATION,
            disk,
        ))
}

fn machine_fact(screen: &ScreenContext<'_>, key: TextKey, value: &str) -> Div {
    machine_fact_value(screen, key, mono(value))
}

fn machine_fact_value(screen: &ScreenContext<'_>, key: TextKey, value: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_w(px(128.))
        .child(muted(screen.palette, key.text(screen.locale)))
        .child(value)
}

fn resource(screen: &ScreenContext<'_>, icon: &'static str, key: TextKey, value: String) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(128.))
        .p_3()
        .gap_2()
        .rounded(px(8.))
        .bg(color(screen.palette, ColorToken::BgSubtle))
        .child(
            row(screen.locale)
                .gap_2()
                .child(system_icon(screen.palette, icon))
                .child(muted(screen.palette, key.text(screen.locale))),
        )
        .child(
            screen
                .typography(marsa_ui::tokens::typography::Role::Label)
                .apply(div())
                .child(technical(&value)),
        )
}

fn cancel_button(id: &'static str, locale: Locale) -> Button {
    Button::new(id)
        .outline()
        .small()
        .label(gui::GUI_DESKTOP_CANCEL.text(locale))
        .on_click(|_, window, cx| window.close_dialog(cx))
}

fn resource_field(label: &'static str, input: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(170.))
        .gap_1()
        .child(
            div()
                .text_size(ui_size(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(input)
}

fn resource_menu<T: Copy + Eq + 'static>(
    id: &'static str,
    title: &'static str,
    locale: Locale,
    value: Rc<Cell<T>>,
    choices: &[T],
    label: impl Fn(T) -> String + Clone + 'static,
) -> impl IntoElement {
    let current = label(value.get());
    let choices = choices.to_vec();
    Button::new(id)
        .outline()
        .accessibility_label(format!("{title} · {current}"))
        .child(
            row(locale)
                .w_full()
                .justify_between()
                .gap_2()
                .child(current)
                .child(Icon::new(IconName::ChevronDown).size(px(14.))),
        )
        .dropdown_menu(move |menu, _, _| {
            choices.iter().fold(menu, |menu, &choice| {
                let value = value.clone();
                menu.item(
                    PopupMenuItem::new(label(choice))
                        .checked(choice == value.get())
                        .on_click(move |_, window, _| {
                            value.set(choice);
                            window.refresh();
                        }),
                )
            })
        })
}
