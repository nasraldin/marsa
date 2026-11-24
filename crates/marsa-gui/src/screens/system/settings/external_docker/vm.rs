//! Machine settings for the exact Lima instance that owns a Docker socket.

use marsa_client::docker::{
    LimaInstance, LimaSettings, LimaSettingsError, LimaSettingsUpdate, ManagedEndpointTransport,
};
use marsa_ui::{
    ColorToken, Locale, TextKey,
    gpui::{
        AppContext as _, Context, Div, Entity, ParentElement as _, Styled as _, Subscription, Task,
        Window, div, px,
    },
    gpui_component::{
        Disableable as _, IconName,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        input::{InputEvent, InputState},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{
    SystemView, confirm_lima_control,
    editor::{Editor, EditorTarget},
    settings_text,
};
use crate::screens::common::{ScreenContext, resource_input, row};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

#[derive(Clone, Copy)]
enum VmWork {
    Inspect,
    Edit(LimaSettings, LimaSettingsUpdate),
    Lifecycle(bool),
    Restart,
}

impl VmWork {
    fn changes_lifecycle(self) -> bool {
        matches!(self, Self::Lifecycle(_) | Self::Restart)
    }
}

#[derive(Clone, Copy)]
enum VmScope {
    Editor,
    Machine,
}

#[derive(Clone, Copy)]
enum VmSection {
    Resources,
    Network,
    Machine,
}

pub(in crate::screens::system) struct VmEditor {
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
        Self::for_instance(instance, window, cx)
    }

    fn for_instance(
        instance: Option<LimaInstance>,
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
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
    pub(crate) fn lima_machine_instance(
        &self,
        workspace: &crate::screens::external_inventory::ExternalWorkspace,
    ) -> Option<LimaInstance> {
        let instance = workspace.endpoint.lima_instance()?;
        if let Some(id) = workspace.endpoint.managed_id()
            && let Some(saved) = &self.environment_settings.saved
        {
            let entry = saved.iter().find(|entry| entry.id == id)?;
            let ManagedEndpointTransport::Unix { socket } = &entry.config.transport else {
                return None;
            };
            if LimaInstance::from_socket(socket).as_ref() != Some(&instance) {
                return None;
            }
        }
        Some(instance)
    }

    pub(crate) fn has_lima_machine_target(&self) -> bool {
        self.machine_lima_target.is_some()
    }

    pub(crate) fn clear_lima_machine_target(&mut self) {
        self.machine_lima_target = None;
        if self.machine_vm.as_ref().is_some_and(|vm| !vm.applying()) {
            self.machine_vm = None;
        }
    }

    pub(crate) fn prepare_lima_machine(
        &mut self,
        instance: Option<LimaInstance>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.machine_lima_target = instance.clone();
        if self
            .machine_vm
            .as_ref()
            .is_some_and(|vm| vm.applying() || vm.instance == instance)
        {
            return;
        }
        self.machine_vm =
            instance.map(|instance| VmEditor::for_instance(Some(instance), window, cx));
        if self.machine_vm.is_some() {
            self.run_lima_vm(VmScope::Machine, VmWork::Inspect, window, cx);
        }
    }

    pub(crate) fn lima_machine(&self, screen: &ScreenContext<'_>, cx: &Context<Self>) -> Div {
        match self.lima_vm(VmScope::Machine) {
            Some(vm) => self.lima_form(vm, VmScope::Machine, VmSection::Machine, screen, cx),
            None => settings_text(
                screen,
                Role::Small,
                gui::GUI_DESKTOP_WORKING.text(screen.locale),
                ColorToken::TextMuted,
            ),
        }
    }

    fn lima_vm(&self, scope: VmScope) -> Option<&VmEditor> {
        match scope {
            VmScope::Editor => self
                .environment_settings
                .editor
                .as_ref()
                .map(|editor| &editor.vm),
            VmScope::Machine => self.machine_vm.as_ref().filter(|vm| {
                self.route == crate::desktop::Route::Machine
                    && self.machine_lima_target.is_some()
                    && vm.instance == self.machine_lima_target
            }),
        }
    }

    fn lima_vm_mut(&mut self, scope: VmScope) -> Option<&mut VmEditor> {
        match scope {
            VmScope::Editor => self
                .environment_settings
                .editor
                .as_mut()
                .map(|editor| &mut editor.vm),
            VmScope::Machine => self.machine_vm.as_mut(),
        }
    }

    fn lima_provider_matches(&self, scope: VmScope, cx: &Context<Self>) -> bool {
        match scope {
            VmScope::Editor => self
                .environment_settings
                .editor
                .as_ref()
                .is_some_and(|editor| editor.provider_matches_draft(cx)),
            VmScope::Machine => self.lima_vm(scope).is_some(),
        }
    }

    pub(super) fn load_environment_vm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_lima_vm(VmScope::Editor, VmWork::Inspect, window, cx);
    }

    fn apply_lima_vm(&mut self, scope: VmScope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(vm) = self.lima_vm(scope) else {
            return;
        };
        let Some(settings) = vm.original.filter(|settings| settings.stopped) else {
            return;
        };
        let update = vm.draft_changes(settings, cx).filter(|update| {
            *update != LimaSettingsUpdate::default()
                && update
                    .disk_bytes
                    .is_none_or(|disk| disk >= settings.disk_bytes)
        });
        if let Some(update) = update {
            self.run_lima_vm(scope, VmWork::Edit(settings, update), window, cx);
        }
    }

    fn request_lima_control(
        &mut self,
        scope: VmScope,
        work: VmWork,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.lima_provider_matches(scope, cx) {
            return;
        }
        if matches!(work, VmWork::Lifecycle(true)) {
            self.run_lima_vm(scope, work, window, cx);
        } else if let Some(vm) = self.lima_vm(scope)
            && let Some(instance) = &vm.instance
        {
            let form = vm.cpus.clone();
            confirm_lima_control(
                instance.name(),
                matches!(work, VmWork::Restart),
                locale,
                window,
                cx,
                move |this, window, cx| {
                    if this.lima_vm(scope).is_some_and(|vm| vm.cpus == form) {
                        this.run_lima_vm(scope, work, window, cx);
                    }
                },
            );
        }
    }

    fn run_lima_vm(
        &mut self,
        scope: VmScope,
        work: VmWork,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lima_vm(scope).is_none()
            || (!matches!(work, VmWork::Inspect) && !self.lima_provider_matches(scope, cx))
        {
            return;
        }
        let Some(services) = self.services.clone() else {
            return;
        };
        let Some(vm) = self.lima_vm_mut(scope) else {
            return;
        };
        let Some(instance) = vm.instance.clone().filter(|_| vm.task.is_none()) else {
            return;
        };
        let form = vm.cpus.clone();
        vm.feedback = None;
        vm.applying = !matches!(work, VmWork::Inspect);
        let mut pending = services.pending(async move {
            Ok(match work {
                VmWork::Inspect => instance.settings().await,
                VmWork::Edit(expected, update) => instance.update(expected, update).await,
                VmWork::Lifecycle(running) => instance.set_running(running).await,
                VmWork::Restart => instance.restart().await,
            })
        });
        vm.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = pending.result().await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(vm) = this.lima_vm_mut(scope).filter(|vm| vm.cpus == form) else {
                        return;
                    };
                    vm.task = None;
                    vm.applying = false;
                    match result {
                        Some(Ok(Ok(settings))) => {
                            vm.show(settings, window, cx);
                            vm.feedback = match work {
                                VmWork::Inspect => None,
                                VmWork::Edit(..) => Some((
                                    gui::GUI_ENVIRONMENT_LIMA_APPLIED,
                                    ColorToken::StatusRunning,
                                )),
                                VmWork::Lifecycle(running) => {
                                    Some((lima_result_key(running), ColorToken::StatusRunning))
                                }
                                VmWork::Restart => Some((
                                    gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_RESTARTED,
                                    ColorToken::StatusRunning,
                                )),
                            };
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
                    if work.changes_lifecycle() {
                        this.environment_settings.checks.clear();
                        this.external_inventory_task = None;
                        this.refresh_external_inventory(cx);
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
        self.lima_form(
            &editor.vm,
            VmScope::Editor,
            if network {
                VmSection::Network
            } else {
                VmSection::Resources
            },
            screen,
            cx,
        )
    }

    fn lima_form(
        &self,
        vm: &VmEditor,
        scope: VmScope,
        section: VmSection,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let busy = vm.task.is_some();
        let provider_matches = self.lima_provider_matches(scope, cx);
        let pending_changes = vm.original.is_some_and(|settings| {
            vm.draft_changes(settings, cx)
                .is_none_or(|changes| changes != LimaSettingsUpdate::default())
        });
        let actions = row(locale)
            .gap_2()
            .flex_wrap()
            .children(
                vm.original
                    .and_then(lima_next_action)
                    .map(|(running, icon, label)| {
                        Button::new("change-lima-lifecycle")
                            .outline()
                            .icon(icon)
                            .label(label.text(locale))
                            .disabled(
                                busy || self.services.is_none()
                                    || !provider_matches
                                    || (running && pending_changes),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_lima_control(
                                    scope,
                                    VmWork::Lifecycle(running),
                                    locale,
                                    window,
                                    cx,
                                );
                            }))
                    }),
            )
            .children(
                (matches!(scope, VmScope::Machine)
                    && vm.original.is_some_and(|settings| settings.running))
                .then(|| {
                    Button::new("restart-lima-machine")
                        .outline()
                        .icon(IconName::RotateCw)
                        .label(gui::GUI_DESKTOP_RESTART_MACHINE.text(locale))
                        .disabled(busy || self.services.is_none() || !provider_matches)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_lima_control(scope, VmWork::Restart, locale, window, cx);
                        }))
                }),
            )
            .child(
                Button::new("refresh-lima-settings")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .tooltip(gui::GUI_DESKTOP_REFRESH.text(locale))
                    .accessibility_label(gui::GUI_DESKTOP_REFRESH.text(locale))
                    .disabled(busy || self.services.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_lima_vm(scope, VmWork::Inspect, window, cx);
                        if matches!(scope, VmScope::Machine) {
                            this.refresh_external_inventory(cx);
                        }
                    })),
            );
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
                            match section {
                                VmSection::Network => gui::GUI_ENVIRONMENT_LIMA_NETWORK,
                                VmSection::Resources | VmSection::Machine => {
                                    gui::GUI_ENVIRONMENT_LIMA_RESOURCES
                                }
                            }
                            .text(locale),
                            ColorToken::TextPrimary,
                        ))
                        .flex_wrap()
                        .child(actions),
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
        let disabled = busy || !settings.stopped || !provider_matches;
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
        if !matches!(section, VmSection::Network) {
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
        if !matches!(section, VmSection::Resources) {
            if let Some(loopback) = vm.loopback {
                body = body
                    .child(
                        Checkbox::new("lima-loopback")
                            .label(gui::GUI_ENVIRONMENT_LIMA_LOOPBACK.text(locale))
                            .checked(loopback)
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, value: &bool, _, cx| {
                                if let Some(vm) = this.lima_vm_mut(scope) {
                                    vm.loopback = Some(*value);
                                    vm.feedback = None;
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
                body = body.child(settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_ENVIRONMENT_LIMA_UNSUPPORTED_NETWORK.text(locale),
                    ColorToken::TextMuted,
                ));
                if matches!(section, VmSection::Network) {
                    return body;
                }
            }
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
        if matches!(scope, VmScope::Editor)
            && self
                .environment_settings
                .editor
                .as_ref()
                .is_some_and(|editor| {
                    matches!(editor.target, EditorTarget::Add | EditorTarget::Saved(..))
                })
        {
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_lima_vm(scope, window, cx)
                    })),
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

pub(super) fn lima_next_action(settings: LimaSettings) -> Option<(bool, IconName, TextKey)> {
    if settings.running {
        Some((
            false,
            IconName::Square,
            gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_STOP,
        ))
    } else if settings.stopped {
        Some((
            true,
            IconName::Play,
            gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_START,
        ))
    } else {
        None
    }
}

pub(super) fn lima_result_key(running: bool) -> TextKey {
    if running {
        gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_STARTED
    } else {
        gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_STOPPED
    }
}

pub(super) fn error_text(error: LimaSettingsError) -> TextKey {
    match error {
        LimaSettingsError::Unavailable => gui::GUI_ENVIRONMENT_LIMA_UNAVAILABLE,
        LimaSettingsError::RequiresStopped => gui::GUI_ENVIRONMENT_LIMA_RUNNING,
        LimaSettingsError::DiskShrink => gui::GUI_ENVIRONMENT_LIMA_NO_SHRINK,
        LimaSettingsError::InvalidResources => gui::GUI_ENVIRONMENT_LIMA_INVALID,
        LimaSettingsError::UnsupportedNetwork => gui::GUI_ENVIRONMENT_LIMA_UNSUPPORTED_NETWORK,
        LimaSettingsError::Changed => gui::GUI_ENVIRONMENT_LIMA_CHANGED,
        LimaSettingsError::LifecycleState => gui::GUI_ENVIRONMENT_LIMA_UNKNOWN_STATE,
        LimaSettingsError::LifecycleFailed => gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_FAILED,
        _ => gui::GUI_ENVIRONMENT_LIMA_FAILED,
    }
}
