//! Resource limits and network preferences backed by the daemon's settings schema.

use crate::screens::common::message;
use marsa_client::api::NetworkMode as ActiveNetworkMode;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken,
    direction::LogicalStyle as _,
    gpui::{AppContext, Context, Div, Entity, ParentElement, Styled, Window, div},
    gpui_component::{
        Disableable, Selectable,
        button::{Button, ButtonVariants},
        checkbox::Checkbox,
        input::{Input, InputState},
    },
    preferences::{NetworkMode, VmBackend},
};

use super::{
    super::{Notice, SystemView},
    settings_card, settings_heading,
};
use crate::screens::common::{ScreenContext, color, label, panel, row, technical};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = MIB * 1024;

pub(in crate::screens::system) struct ResourceInputs {
    pub(in crate::screens::system) open: bool,
    cpus: Entity<InputState>,
    memory: Entity<InputState>,
    disk: Entity<InputState>,
    timeout: Entity<InputState>,
    dns: Entity<InputState>,
    http: Entity<InputState>,
    https: Entity<InputState>,
}

impl ResourceInputs {
    pub(in crate::screens::system) fn new(
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
        let mut input = || cx.new(|cx| InputState::new(window, cx));
        Self {
            open: false,
            cpus: input(),
            memory: input(),
            disk: input(),
            timeout: input(),
            dns: input(),
            http: input(),
            https: input(),
        }
    }
}

impl SystemView {
    pub(in crate::screens::system) fn show_resource_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((settings, _)) = &self.settings else {
            return;
        };
        let inputs = &self.resource_inputs;
        let dns = settings
            .network
            .dns
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for (input, value) in [
            (&inputs.cpus, settings.vm.cpus.to_string()),
            (
                &inputs.memory,
                settings
                    .vm
                    .memory
                    .map(|value| (value / MIB).to_string())
                    .unwrap_or_default(),
            ),
            (
                &inputs.disk,
                (settings.disk.data_size_limit / GIB).to_string(),
            ),
            (&inputs.timeout, settings.vm.idle_timeout.to_string()),
            (&inputs.dns, dns),
            (
                &inputs.http,
                settings.network.proxy.http.clone().unwrap_or_default(),
            ),
            (
                &inputs.https,
                settings.network.proxy.https.clone().unwrap_or_default(),
            ),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.resource_inputs.open = true;
        cx.notify();
    }

    pub(in crate::screens::system) fn resource_editor(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some((settings, _)) = &self.settings else {
            return div().child(message(
                screen.locale,
                gui::GUI_DESKTOP_WAITING_FOR_MACHINE_SETTINGS,
            ));
        };
        let inputs = &self.resource_inputs;
        if !inputs.open {
            return div().child(
                Button::new("open-resource-editor")
                    .outline()
                    .label(message(screen.locale, gui::GUI_DESKTOP_EDIT_RESOURCES))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_resource_inputs(window, cx)),
                    ),
            );
        }
        let locale = screen.locale;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                row(locale)
                    .gap_3()
                    .child(field(screen, "Processors", "المعالجات", &inputs.cpus))
                    .child(field(
                        screen,
                        "Memory (MiB, blank for automatic)",
                        "الذاكرة (MiB، اتركه فارغًا للضبط التلقائي)",
                        &inputs.memory,
                    )),
            )
            .child(
                row(locale)
                    .gap_3()
                    .child(field(
                        screen,
                        "Disk limit (GiB)",
                        "حدّ القرص (GiB)",
                        &inputs.disk,
                    ))
                    .child(field(
                        screen,
                        "Idle timeout (seconds)",
                        "مهلة الخمول (بالثواني)",
                        &inputs.timeout,
                    )),
            )
            .child(
                row(locale)
                    .gap_2()
                    .child(message(screen.locale, gui::GUI_DESKTOP_BACKEND))
                    .children(
                        [
                            (VmBackend::Vz, "Virtualization.framework"),
                            (VmBackend::Krun, "libkrun"),
                        ]
                        .into_iter()
                        .map(|(value, text)| {
                            Button::new(("machine-backend", value as usize))
                                .outline()
                                .selected(settings.vm.backend == value)
                                .disabled(self.busy())
                                .label(text)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_settings(|settings| settings.vm.backend = value, cx)
                                }))
                        }),
                    ),
            )
            .child(
                row(locale)
                    .gap_3()
                    .child(
                        Checkbox::new("idle-stop")
                            .checked(settings.vm.idle_stop)
                            .label(message(
                                screen.locale,
                                gui::GUI_DESKTOP_STOP_THE_MACHINE_WHEN_IDLE,
                            ))
                            .disabled(self.busy())
                            .on_click(cx.listener(|this, value: &bool, _, cx| {
                                this.save_settings(|settings| settings.vm.idle_stop = *value, cx)
                            })),
                    )
                    .child(
                        Checkbox::new("disk-reclaim-pref")
                            .checked(settings.disk.auto_reclaim)
                            .label(message(
                                screen.locale,
                                gui::GUI_DESKTOP_RECLAIM_DISK_SPACE_AUTOMATICALLY,
                            ))
                            .disabled(self.busy())
                            .on_click(cx.listener(|this, value: &bool, _, cx| {
                                this.save_settings(
                                    |settings| settings.disk.auto_reclaim = *value,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Checkbox::new("disk-backup-pref")
                            .checked(settings.disk.exclude_from_backup)
                            .label(message(
                                screen.locale,
                                gui::GUI_DESKTOP_EXCLUDE_DISK_FROM_TIME_MACHINE,
                            ))
                            .disabled(self.busy())
                            .on_click(cx.listener(|this, value: &bool, _, cx| {
                                this.save_settings(
                                    |settings| settings.disk.exclude_from_backup = *value,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(
                row(locale).justify_row_end().gap_2().child(
                    Button::new("save-machine-resources")
                        .primary()
                        .disabled(self.busy())
                        .label(message(screen.locale, gui::GUI_DESKTOP_SAVE_RESOURCES))
                        .on_click(cx.listener(|this, _, _, cx| this.save_resources(cx))),
                ),
            )
    }

    fn save_resources(&mut self, cx: &mut Context<Self>) {
        let inputs = &self.resource_inputs;
        let text = |input: &Entity<InputState>| input.read(cx).value();
        let parse = || {
            let cpus = text(&inputs.cpus).trim().parse::<u16>().ok()?;
            let memory = text(&inputs.memory);
            let memory = if memory.trim().is_empty() {
                None
            } else {
                Some(memory.trim().parse::<u64>().ok()?.checked_mul(MIB)?)
            };
            let disk = text(&inputs.disk)
                .trim()
                .parse::<u64>()
                .ok()?
                .checked_mul(GIB)?;
            let timeout = text(&inputs.timeout).trim().parse::<u32>().ok()?;
            Some((cpus, memory, disk, timeout))
        };
        let Some((cpus, memory, disk, timeout)) = parse() else {
            self.notice = Some(Notice::InvalidSettings);
            cx.notify();
            return;
        };
        self.save_settings(
            |settings| {
                settings.vm.cpus = cpus;
                settings.vm.memory = memory;
                settings.vm.idle_timeout = timeout;
                settings.disk.data_size_limit = disk;
            },
            cx,
        );
    }

    pub(super) fn network(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let Some((settings, _)) = &self.settings else {
            return panel(screen).child(message(
                screen.locale,
                gui::GUI_DESKTOP_WAITING_FOR_NETWORK_SETTINGS,
            ));
        };
        let network = screen
            .state
            .daemon()
            .and_then(|snapshot| snapshot.network.as_ref());
        let locale = screen.locale;
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DESKTOP_NETWORK_MODE.text(locale),
                gui::GUI_DESKTOP_AUTOMATIC_PREFERS_NATIVE_NAT_AND_FALLS_BACK_WHEN_EGRESS_IS_BLO
                    .text(locale),
            ))
            .child(
                row(locale).gap_2().children(
                    [
                        (
                            NetworkMode::Auto,
                            message(screen.locale, gui::GUI_DESKTOP_AUTOMATIC_52B32AC9),
                        ),
                        (
                            NetworkMode::Nat,
                            message(screen.locale, gui::GUI_DESKTOP_NATIVE_NAT),
                        ),
                        (
                            NetworkMode::Userspace,
                            message(screen.locale, gui::GUI_DESKTOP_USERSPACE),
                        ),
                    ]
                    .into_iter()
                    .map(|(mode, name)| {
                        Button::new(("network-mode", mode as usize))
                            .outline()
                            .selected(settings.network.mode == mode)
                            .disabled(self.busy())
                            .label(name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_settings(|settings| settings.network.mode = mode, cx)
                            }))
                    }),
                ),
            )
            .children(network.map(|network| {
                let mode = match network.active_mode() {
                    ActiveNetworkMode::Nat => gui::GUI_DESKTOP_NATIVE_NAT,
                    ActiveNetworkMode::Userspace => gui::GUI_DESKTOP_USERSPACE,
                    _ => gui::GUI_DESKTOP_NETWORK_NO_ACTIVE_MODE,
                };
                row(locale)
                    .gap_2()
                    .child(message(locale, gui::GUI_DESKTOP_NETWORK_ACTIVE_MODE))
                    .child(message(locale, mode))
            }))
            .children(
                network
                    .is_some_and(|network| network.restart_pending == Some(true))
                    .then(|| message(locale, gui::GUI_DESKTOP_NETWORK_RESTART_PENDING)),
            )
            .children(
                network
                    .and_then(|network| network.guest_address.as_deref())
                    .map(|address| {
                        row(locale)
                            .gap_2()
                            .child(message(screen.locale, gui::GUI_DESKTOP_GUEST_ADDRESS))
                            .child(technical(address))
                    }),
            )
            .child(
                row(locale).child(
                    Checkbox::new("allow-lan-published-ports")
                        .label(message(locale, gui::GUI_DESKTOP_ALLOW_LAN_PUBLISHED_PORTS))
                        .checked(settings.network.allow_lan)
                        .disabled(self.busy())
                        .on_click(cx.listener(|this, enabled: &bool, _, cx| {
                            this.save_settings(|settings| settings.network.allow_lan = *enabled, cx)
                        })),
                ),
            )
            .child(self.proxy_controls(screen, settings.network.proxy.inject_into_containers, cx))
            .children(self.resource_inputs.open.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(field(
                        screen,
                        "DNS servers (comma separated)",
                        "خوادم DNS (مفصولة بفاصلة)",
                        &self.resource_inputs.dns,
                    ))
                    .child(
                        row(locale)
                            .gap_3()
                            .child(field(
                                screen,
                                "HTTP proxy",
                                "وكيل HTTP",
                                &self.resource_inputs.http,
                            ))
                            .child(field(
                                screen,
                                "HTTPS proxy",
                                "وكيل HTTPS",
                                &self.resource_inputs.https,
                            )),
                    )
                    .child(
                        row(locale).justify_row_end().child(
                            Button::new("save-network-overrides")
                                .primary()
                                .disabled(self.busy())
                                .label(message(screen.locale, gui::GUI_DESKTOP_SAVE_OVERRIDES))
                                .on_click(cx.listener(|this, _, _, cx| this.save_network(cx))),
                        ),
                    )
            }))
            .children((!self.resource_inputs.open).then(|| {
                Button::new("edit-network-overrides")
                    .outline()
                    .label(message(screen.locale, gui::GUI_DESKTOP_EDIT_OVERRIDES))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_resource_inputs(window, cx)),
                    )
            }))
    }

    fn save_network(&mut self, cx: &mut Context<Self>) {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let dns = text(&self.resource_inputs.dns);
        let parsed = if dns.is_empty() {
            Ok(Vec::new())
        } else {
            dns.split(',')
                .map(|address| address.trim().parse())
                .collect()
        };
        let Ok(dns) = parsed else {
            self.notice = Some(Notice::InvalidSettings);
            cx.notify();
            return;
        };
        let override_value = |input| {
            let value = text(input);
            (!value.is_empty()).then_some(value)
        };
        let http = override_value(&self.resource_inputs.http);
        let https = override_value(&self.resource_inputs.https);
        self.save_settings(
            |settings| {
                settings.network.dns = dns;
                settings.network.proxy.http = http;
                settings.network.proxy.https = https;
            },
            cx,
        );
    }
}

fn field(
    screen: &ScreenContext<'_>,
    en: &'static str,
    ar: &'static str,
    input: &Entity<InputState>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(label(screen, en, ar)),
        )
        .child(Input::new(input).aria_label(label(screen, en, ar)))
}
