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
    screens::common::{ScreenContext, align, color, message, panel, row, technical},
    services::{Command, EngineAction},
};

use super::{Notice, SystemView, disk::bytes};

const GIB: u64 = 1024 * 1024 * 1024;

impl SystemView {
    pub(crate) fn engine(
        &mut self,
        action: EngineAction,
        locale: Locale,
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
            dialog
                .centered()
                .width(px(570.))
                .overlay_closable(false)
                .title(dialog_title(title.text(locale)))
                .child(
                    align(div().flex().flex_col().gap_5(), locale)
                        .child(explanation.text(locale))
                        .child(gui::GUI_SYSTEM_MACHINE_VOLUMES_STAY.text(locale)),
                )
                .footer(
                    row(locale)
                        .justify_end()
                        .gap_2()
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
            row(locale)
                .gap_4()
                .child(resource(
                    screen,
                    "icons/cpu.svg",
                    gui::GUI_SYSTEM_MACHINE_PROCESSORS,
                    format!("{} vCPU", settings.vm.cpus),
                ))
                .child(resource(
                    screen,
                    "icons/memory-stick.svg",
                    gui::GUI_SYSTEM_MACHINE_MEMORY,
                    settings
                        .vm
                        .memory
                        .map(bytes)
                        .unwrap_or_else(|| gui::GUI_DESKTOP_AUTOMATIC.text(locale).to_owned()),
                ))
                .child(resource(
                    screen,
                    "icons/hard-drive.svg",
                    gui::GUI_SYSTEM_MACHINE_DISK_ALLOCATION,
                    self.disk
                        .as_ref()
                        .map(|disk| bytes(disk.host_allocated_bytes))
                        .unwrap_or_else(|| gui::GUI_DESKTOP_UNAVAILABLE.text(locale).to_owned()),
                ))
        });
        let boot = startup_copy(state);
        let detail = engine.cloned();
        div().flex().flex_col().gap_4()
            .child(panel(screen).gap_4().pt_3()
                .child(row(locale).justify_between().items_start().gap_3()
                    .child(div().flex().flex_col().gap_4()
                        .child(row(locale).gap_2()
                            .child(system_icon(screen.palette, "icons/server.svg"))
                            .child(title_text(gui::GUI_DESKTOP_LINUX_VIRTUAL_MACHINE.text(locale))))
                        .child(row(locale).child(status::compact_badge(locale, palette, state))))
                    .child(row(locale).gap_2().children(controls.iter().copied().map(|action| {
                        let (id, key) = match action {
                            EngineAction::Start => ("start-machine", gui::GUI_DESKTOP_START_MACHINE),
                            EngineAction::Stop => ("stop-machine", gui::GUI_DESKTOP_STOP_MACHINE),
                            EngineAction::Restart => ("restart-machine", gui::GUI_DESKTOP_RESTART_MACHINE),
                        };
                        Button::new(id).outline().h(px(44.)).disabled(self.busy()).label(key.text(locale))
                            .on_click(cx.listener(move |view, _, window, cx| view.engine(action, locale, window, cx)))
                    }))))
                .child(row(locale).gap_4().border_t_1().pt_2()
                    .border_color(color(palette, ColorToken::BorderSubtle))
                    .child(machine_fact(screen, gui::GUI_DESKTOP_BACKEND, backend))
                    .child(machine_fact(screen, gui::GUI_SYSTEM_MACHINE_ARCHITECTURE, host_architecture()))
                    .child(machine_fact(screen, gui::GUI_DESKTOP_DOCKER_CONTEXT, if engine.is_some() { "marsa" } else { "—" })))
                .children(running.then(|| muted(palette, gui::GUI_DESKTOP_RUNNING_WORKLOADS_WILL_BE_INTERRUPTED_SAVE_YOUR_WORK_FIRST.text(locale)))),
            )
            .child(panel(screen).gap_4().pt_3()
                .child(div().flex().flex_col().gap_1()
                    .child(title_text(gui::GUI_DESKTOP_RESOURCES.text(locale)))
                    .child(muted(palette, gui::GUI_SYSTEM_MACHINE_RESOURCE_NOTICE.text(locale))))
                .children(resources)
                .child(if self.restart_required {
                    div().text_size(ui_size(14.)).text_color(color(palette, ColorToken::AccentDefault))
                        .child(gui::GUI_DESKTOP_RESOURCE_CHANGES_ARE_PENDING.text(locale))
                } else {
                    row(locale).justify_end().child(Button::new("edit-machine-resources").outline().h(px(44.))
                        .disabled(self.settings.is_none() || self.busy())
                        .label(gui::GUI_DESKTOP_EDIT_RESOURCES.text(locale))
                        .on_click(cx.listener(move |view, _, window, cx| view.edit_machine_resources(locale, palette, window, cx))))
                }),
            )
            .child(panel(screen).child(row(locale).justify_between().gap_4()
                .child(div().flex().flex_col().gap_1()
                    .child(title_text(gui::GUI_DESKTOP_BOOT_DIAGNOSTICS.text(locale)))
                    .child(muted(palette, boot.text(locale)))
                    .children(matches!(state, Some(EngineState::Starting | EngineState::Booting | EngineState::Failed))
                        .then(|| engine.and_then(|engine| engine.boot_stage.as_deref())).flatten().map(mono)))
                .child(Button::new("startup-details").outline().h(px(44.))
                    .label(gui::GUI_DESKTOP_VIEW_STARTUP_DETAILS.text(locale))
                    .on_click(cx.listener(move |view, _, window, cx| view.startup_details(detail.clone(), locale, palette, window, cx))))),
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
                panel(screen)
                    .rounded(px(8.))
                    .min_h(px(126.))
                    .child(title_text(gui::GUI_SYSTEM_MACHINE_BOOT_STAGES.text(locale)))
                    .child(row(locale).gap_4().py_3().children(
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
                                .min_w_0()
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
                    .min_h(px(198.))
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
            .child(
                row(locale)
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("failed-machine-diagnostics")
                            .outline()
                            .icon(Icon::default().path("icons/download.svg"))
                            .accessibility_label(gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS.text(locale))
                            .tooltip(gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS.text(locale))
                            .disabled(self.busy() || self.services.is_none())
                            .on_click(cx.listener(|view, _, _, cx| view.collect_diagnostics(cx))),
                    )
                    .child(
                        Button::new("retry-failed-machine")
                            .primary()
                            .icon(Icon::default().path("icons/refresh-cw.svg"))
                            .accessibility_label(gui::GUI_DESKTOP_RETRY.text(locale))
                            .tooltip(gui::GUI_DESKTOP_RETRY.text(locale))
                            .disabled(self.busy() || self.services.is_none())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.run_engine(EngineAction::Start, cx)
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
            let fields = align(div().flex().flex_col().gap_5(), locale)
                .child(muted(
                    palette,
                    gui::GUI_SYSTEM_MACHINE_AFTER_NEXT_RESTART.text(locale),
                ))
                .child(
                    row(locale)
                        .justify_between()
                        .gap_4()
                        .child(gui::GUI_SYSTEM_MACHINE_PROCESSORS.text(locale))
                        .child(resource_menu(
                            "machine-cpu-count",
                            cpus.clone(),
                            &cpu_choices,
                            |cpu| format!("{cpu} vCPU"),
                        )),
                )
                .child(
                    row(locale)
                        .justify_between()
                        .gap_4()
                        .child(gui::GUI_SYSTEM_MACHINE_MEMORY.text(locale))
                        .child(resource_menu(
                            "machine-memory-size",
                            memory.clone(),
                            &memory_choices,
                            move |memory| {
                                memory.map(bytes).unwrap_or_else(|| {
                                    gui::GUI_DESKTOP_AUTOMATIC.text(locale).to_owned()
                                })
                            },
                        )),
                );
            dialog
                .centered()
                .width(px(576.))
                .overlay_closable(false)
                .title(dialog_title(gui::GUI_DESKTOP_EDIT_RESOURCES.text(locale)))
                .child(fields)
                .footer(
                    row(locale)
                        .justify_end()
                        .gap_2()
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
            dialog
                .centered()
                .width(px(660.))
                .title(dialog_title(gui::GUI_DESKTOP_BOOT_DIAGNOSTICS.text(locale)))
                .child(
                    align(div().flex().flex_col().gap_5(), locale)
                        .child(muted(palette, context.text(locale)))
                        .child(
                            div()
                                .id("machine-startup-output")
                                .overflow_y_scrollbar()
                                .min_h(px(128.))
                                .max_h(px(252.))
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
                    row(locale)
                        .justify_end()
                        .gap_2()
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

pub(super) fn title_text(text: &'static str) -> Div {
    div()
        .text_size(ui_size(16.))
        .line_height(ui_size(24.))
        .font_weight(FontWeight::MEDIUM)
        .child(text)
}

pub(super) fn dialog_title(text: &'static str) -> Div {
    div()
        .text_size(ui_size(27.))
        .line_height(ui_size(36.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
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

fn machine_fact(screen: &ScreenContext<'_>, key: TextKey, value: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_w_0()
        .child(muted(screen.palette, key.text(screen.locale)))
        .child(mono(value))
}

fn resource(screen: &ScreenContext<'_>, icon: &'static str, key: TextKey, value: String) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .p_3()
        .gap_3()
        .rounded(px(8.))
        .bg(color(screen.palette, ColorToken::BgSubtle))
        .child(
            row(screen.locale)
                .gap_2()
                .child(system_icon(screen.palette, icon))
                .child(muted(screen.palette, key.text(screen.locale))),
        )
        .child(mono(&value))
}

fn cancel_button(id: &'static str, locale: Locale) -> Button {
    Button::new(id)
        .outline()
        .small()
        .label(gui::GUI_DESKTOP_CANCEL.text(locale))
        .on_click(|_, window, cx| window.close_dialog(cx))
}

fn resource_menu<T: Copy + Eq + 'static>(
    id: &'static str,
    value: Rc<Cell<T>>,
    choices: &[T],
    label: impl Fn(T) -> String + Clone + 'static,
) -> impl IntoElement {
    let current = value.get();
    let choices = choices.to_vec();
    Button::new(id)
        .outline()
        .icon(IconName::ChevronDown)
        .label(label(current))
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
