//! Machine settings for the exact Lima instance that owns a Docker socket.

use marsa_client::docker::{
    LimaInstance, LimaSettings, LimaSettingsError, LimaSettingsUpdate, ManagedEndpointTransport,
};
use marsa_ui::{
    ColorToken, TextKey,
    gpui::{
        AppContext as _, Context, Div, Entity, ParentElement as _, Styled as _, Subscription, Task,
        Window, div, px,
    },
    gpui_component::{
        Disableable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        input::{InputEvent, InputState},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{
    SystemView,
    editor::{Editor, EditorTarget},
    settings_text,
};
use crate::screens::common::{ScreenContext, resource_input, row};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

pub(super) struct VmEditor {
    instance: Option<LimaInstance>,
    original: Option<LimaSettings>,
    cpus: Entity<InputState>,
    memory: Entity<InputState>,
    disk: Entity<InputState>,
    loopback: Option<bool>,
    task: Option<Task<()>>,
    applying: bool,
    feedback: Option<(TextKey, ColorToken)>,
    _input_changes: [Subscription; 3],
}

impl VmEditor {
    pub(super) fn new(
        target: &EditorTarget,
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
        let instance = match target {
            EditorTarget::Discovered(workspace) => workspace
                .endpoint
                .socket()
                .and_then(LimaInstance::from_socket),
            EditorTarget::Saved(entry, _) => match &entry.config.transport {
                ManagedEndpointTransport::Unix { socket } => LimaInstance::from_socket(socket),
                _ => None,
            },
            _ => None,
        };
        let [cpus, memory, disk] =
            std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));
        let changes = [&cpus, &memory, &disk].map(|field| {
            cx.subscribe(field, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.refresh_windows();
                }
            })
        });
        Self {
            instance,
            original: None,
            cpus,
            memory,
            disk,
            loopback: None,
            task: None,
            applying: false,
            feedback: None,
            _input_changes: changes,
        }
    }

    pub(super) fn available(&self) -> bool {
        self.instance.is_some()
    }

    pub(super) fn applying(&self) -> bool {
        self.applying
    }

    fn show(&mut self, settings: LimaSettings, window: &mut Window, cx: &mut Context<SystemView>) {
        self.original = Some(settings);
        self.loopback = settings.host_loopback;
        for (field, value) in [
            (&self.cpus, settings.cpus.to_string()),
            (&self.memory, amount(settings.memory_bytes, MIB)),
            (&self.disk, amount(settings.disk_bytes, GIB)),
        ] {
            field.update(cx, |field, cx| field.set_value(value, window, cx));
        }
    }

    fn draft_changes(
        &self,
        settings: LimaSettings,
        cx: &Context<SystemView>,
    ) -> Option<LimaSettingsUpdate> {
        let cpus = self
            .cpus
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|cpus| *cpus != 0)?;
        let memory = resource_bytes(
            self.memory.read(cx).value().trim(),
            settings.memory_bytes,
            MIB,
        )?;
        let disk = resource_bytes(self.disk.read(cx).value().trim(), settings.disk_bytes, GIB)?;
        Some(LimaSettingsUpdate {
            cpus: (cpus != settings.cpus).then_some(cpus),
            memory_bytes: (memory != settings.memory_bytes).then_some(memory),
            disk_bytes: (disk != settings.disk_bytes).then_some(disk),
            host_loopback: self
                .loopback
                .filter(|value| Some(*value) != settings.host_loopback),
        })
    }
}

impl SystemView {
    pub(super) fn load_environment_vm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_environment_vm(None, window, cx);
    }

    fn apply_environment_vm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.environment_settings.editor else {
            return;
        };
        let vm = &editor.vm;
        let Some(settings) = vm.original.filter(|settings| settings.stopped) else {
            return;
        };
        if !editor.provider_matches_draft(cx) {
            return;
        }
        let update = vm.draft_changes(settings, cx).filter(|update| {
            *update != LimaSettingsUpdate::default()
                && update
                    .disk_bytes
                    .is_none_or(|disk| disk >= settings.disk_bytes)
        });
        if let Some(update) = update {
            self.run_environment_vm(Some((settings, update)), window, cx);
        }
    }

    fn run_environment_vm(
        &mut self,
        changes: Option<(LimaSettings, LimaSettingsUpdate)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.environment_settings.editor else {
            return;
        };
        let Some(services) = &self.services else {
            return;
        };
        let vm = &mut editor.vm;
        let Some(instance) = vm.instance.clone().filter(|_| vm.task.is_none()) else {
            return;
        };
        vm.feedback = None;
        vm.applying = changes.is_some();
        let mut pending = services.pending(async move {
            Ok(match changes {
                Some((expected, update)) => instance.update(expected, update).await,
                None => instance.settings().await,
            })
        });
        vm.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = pending.result().await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(editor) = &mut this.environment_settings.editor else {
                        return;
                    };
                    let vm = &mut editor.vm;
                    vm.task = None;
                    vm.applying = false;
                    match result {
                        Some(Ok(Ok(settings))) => {
                            vm.show(settings, window, cx);
                            if changes.is_some() {
                                vm.feedback = Some((
                                    gui::GUI_ENVIRONMENT_LIMA_APPLIED,
                                    ColorToken::StatusRunning,
                                ));
                            }
                        }
                        Some(Ok(Err(error))) => {
                            if !matches!(
                                error,
                                LimaSettingsError::InvalidResources | LimaSettingsError::DiskShrink
                            ) {
                                vm.original = None;
                            }
                            vm.feedback = Some((error_text(error), ColorToken::StatusWarning));
                        }
                        _ => {
                            vm.original = None;
                            vm.feedback =
                                Some((gui::GUI_ENVIRONMENT_LIMA_FAILED, ColorToken::StatusWarning));
                        }
                    }
                    cx.notify();
                });
                window.refresh();
            });
        }));
        cx.notify();
    }

    pub(super) fn environment_vm(
        &self,
        editor: &Editor,
        network: bool,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let vm = &editor.vm;
        let locale = screen.locale;
        let busy = vm.task.is_some();
        let mut body =
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    row(locale)
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(settings_text(
                            screen,
                            Role::Lead,
                            if network {
                                gui::GUI_ENVIRONMENT_LIMA_NETWORK
                            } else {
                                gui::GUI_ENVIRONMENT_LIMA_RESOURCES
                            }
                            .text(locale),
                            ColorToken::TextPrimary,
                        ))
                        .child(
                            Button::new("refresh-lima-settings")
                                .outline()
                                .label(gui::GUI_DESKTOP_REFRESH.text(locale))
                                .disabled(busy || self.services.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.load_environment_vm(window, cx)
                                })),
                        ),
                )
                .children(vm.feedback.map(|(key, token)| {
                    settings_text(screen, Role::Small, key.text(locale), token)
                }))
                .children(busy.then(|| {
                    settings_text(
                        screen,
                        Role::Small,
                        gui::GUI_DESKTOP_WORKING.text(locale),
                        ColorToken::TextMuted,
                    )
                }));
        let Some(settings) = vm.original else {
            return body;
        };
        let disabled = busy || !settings.stopped;
        body = body.child(settings_text(
            screen,
            Role::Small,
            if settings.stopped {
                gui::GUI_ENVIRONMENT_LIMA_STOPPED
            } else if settings.running {
                gui::GUI_ENVIRONMENT_LIMA_RUNNING
            } else {
                gui::GUI_ENVIRONMENT_LIMA_UNKNOWN_STATE
            }
            .text(locale),
            if settings.stopped {
                ColorToken::TextMuted
            } else {
                ColorToken::StatusWarning
            },
        ));
        if network {
            if let Some(loopback) = vm.loopback {
                body = body
                    .child(
                        Checkbox::new("lima-loopback")
                            .label(gui::GUI_ENVIRONMENT_LIMA_LOOPBACK.text(locale))
                            .checked(loopback)
                            .disabled(disabled)
                            .on_click(cx.listener(|this, value: &bool, _, cx| {
                                if let Some(editor) = &mut this.environment_settings.editor {
                                    editor.vm.loopback = Some(*value);
                                    editor.vm.feedback = None;
                                }
                                cx.notify();
                            })),
                    )
                    .child(settings_text(
                        screen,
                        Role::Small,
                        gui::GUI_ENVIRONMENT_LIMA_LOOPBACK_HELP.text(locale),
                        ColorToken::TextMuted,
                    ));
            } else {
                return body.child(settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_ENVIRONMENT_LIMA_UNSUPPORTED_NETWORK.text(locale),
                    ColorToken::TextMuted,
                ));
            }
        } else {
            body = body
                .child(
                    row(locale)
                        .gap_3()
                        .flex_wrap()
                        .child(field(
                            screen,
                            gui::GUI_ENVIRONMENT_LIMA_CPUS,
                            &vm.cpus,
                            disabled,
                        ))
                        .child(field(
                            screen,
                            gui::GUI_ENVIRONMENT_LIMA_RAM,
                            &vm.memory,
                            disabled,
                        ))
                        .child(field(
                            screen,
                            gui::GUI_ENVIRONMENT_LIMA_DISK,
                            &vm.disk,
                            disabled,
                        )),
                )
                .child(settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_ENVIRONMENT_LIMA_DISK_HELP.text(locale),
                    ColorToken::TextMuted,
                ));
        }
        let changes = vm.draft_changes(settings, cx);
        let shrinks = changes
            .and_then(|changes| changes.disk_bytes)
            .is_some_and(|disk| disk < settings.disk_bytes);
        let changed = changes.is_some_and(|changes| changes != LimaSettingsUpdate::default());
        if shrinks {
            body = body.child(settings_text(
                screen,
                Role::Small,
                gui::GUI_ENVIRONMENT_LIMA_NO_SHRINK.text(locale),
                ColorToken::StatusWarning,
            ));
        } else if settings.stopped && changes.is_none() {
            body = body.child(settings_text(
                screen,
                Role::Small,
                gui::GUI_ENVIRONMENT_LIMA_INVALID.text(locale),
                ColorToken::StatusWarning,
            ));
        }
        if matches!(editor.target, EditorTarget::Add | EditorTarget::Saved(..)) {
            body = body.child(settings_text(
                screen,
                Role::Small,
                gui::GUI_ENVIRONMENT_LIMA_INDEPENDENT.text(locale),
                ColorToken::TextMuted,
            ));
        }
        body.child(
            row(locale).child(
                Button::new("apply-lima-settings")
                    .primary()
                    .label(gui::GUI_ENVIRONMENT_LIMA_APPLY.text(locale))
                    .disabled(disabled || shrinks || !changed)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.apply_environment_vm(window, cx)),
                    ),
            ),
        )
    }
}

fn amount(bytes: u64, unit: u64) -> String {
    if bytes.is_multiple_of(unit) {
        (bytes / unit).to_string()
    } else {
        format!("{:.3}", bytes as f64 / unit as f64)
    }
}

fn resource_bytes(value: &str, original: u64, unit: u64) -> Option<u64> {
    if value == amount(original, unit) {
        return Some(original);
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|v| *v != 0)?
        .checked_mul(unit)
}

fn field(
    screen: &ScreenContext<'_>,
    label: TextKey,
    state: &Entity<InputState>,
    disabled: bool,
) -> Div {
    let label = label.text(screen.locale);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(140.))
        .flex_1()
        .child(settings_text(
            screen,
            Role::Label,
            label,
            ColorToken::TextPrimary,
        ))
        .child(resource_input(state).aria_label(label).disabled(disabled))
}

fn error_text(error: LimaSettingsError) -> TextKey {
    match error {
        LimaSettingsError::Unavailable => gui::GUI_ENVIRONMENT_LIMA_UNAVAILABLE,
        LimaSettingsError::RequiresStopped => gui::GUI_ENVIRONMENT_LIMA_RUNNING,
        LimaSettingsError::DiskShrink => gui::GUI_ENVIRONMENT_LIMA_NO_SHRINK,
        LimaSettingsError::InvalidResources => gui::GUI_ENVIRONMENT_LIMA_INVALID,
        LimaSettingsError::UnsupportedNetwork => gui::GUI_ENVIRONMENT_LIMA_UNSUPPORTED_NETWORK,
        LimaSettingsError::Changed => gui::GUI_ENVIRONMENT_LIMA_CHANGED,
        _ => gui::GUI_ENVIRONMENT_LIMA_FAILED,
    }
}
