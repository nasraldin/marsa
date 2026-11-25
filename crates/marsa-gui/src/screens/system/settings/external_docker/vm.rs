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
        Disableable as _, Icon, IconName, WindowExt as _,
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
    editing: bool,
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
            editing: false,
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
        match &mut self.machine_vm {
            Some(vm) if vm.applying() => vm.editing = false,
            _ => self.machine_vm = None,
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

    pub(in crate::screens::system) fn lima_machine_settings(&self) -> Option<LimaSettings> {
        self.lima_vm(VmScope::Machine)?.original
    }

    pub(crate) fn lima_machine_state(&self) -> Option<marsa_client::api::EngineState> {
        use marsa_client::api::EngineState;
        let settings = self.lima_machine_settings()?;
        match (settings.running, settings.stopped) {
            (true, false) => Some(EngineState::Ready),
            (false, true) => Some(EngineState::Stopped),
            _ => None,
        }
    }

    pub(crate) fn overview_lima_power(
        &self,
        locale: Locale,
        icon_only: bool,
        cx: &Context<Self>,
    ) -> Option<Button> {
        let vm = self.lima_vm(VmScope::Machine)?;
        self.lima_power_button(vm, VmScope::Machine, locale, icon_only, cx)
    }

    pub(crate) fn refresh_lima_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_lima_vm(VmScope::Machine, VmWork::Inspect, window, cx);
        self.refresh_external_inventory(cx);
    }

    pub(in crate::screens::system) fn lima_machine_actions(
        &self,
        locale: Locale,
        cx: &Context<Self>,
    ) -> Div {
        let vm = self.lima_vm(VmScope::Machine);
        row(locale)
            .gap_2()
            .flex_wrap()
            .children(
                vm.and_then(|vm| self.lima_power_button(vm, VmScope::Machine, locale, false, cx)),
            )
            .children(
                vm.filter(|vm| vm.original.is_some_and(|settings| settings.running))
                    .map(|vm| {
                        Button::new("restart-lima-machine")
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(gui::GUI_DESKTOP_RESTART_MACHINE.text(locale))
                            .disabled(vm.task.is_some() || self.services.is_none())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_lima_control(
                                    VmScope::Machine,
                                    VmWork::Restart,
                                    locale,
                                    window,
                                    cx,
                                );
                            }))
                    }),
            )
    }

    pub(in crate::screens::system) fn lima_machine_edit_button(
        &self,
        locale: Locale,
        cx: &Context<Self>,
    ) -> Button {
        let vm = self.lima_vm(VmScope::Machine);
        let editing = vm.is_some_and(|vm| vm.editing);
        Button::new("edit-lima-machine-resources")
            .outline()
            .disabled(vm.is_none_or(|vm| {
                vm.task.is_some()
                    || (!editing && vm.original.is_none_or(|settings| !settings.stopped))
            }))
            .label(if editing {
                gui::GUI_DESKTOP_CANCEL.text(locale)
            } else {
                gui::GUI_DESKTOP_EDIT_RESOURCES.text(locale)
            })
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(vm) = this.lima_vm_mut(VmScope::Machine) {
                    vm.editing = !vm.editing;
                    if !vm.editing
                        && let Some(settings) = vm.original
                    {
                        vm.show(settings, window, cx);
                    }
                    cx.notify();
                }
            }))
    }

    pub(in crate::screens::system) fn lima_machine_editor(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let vm = self.lima_vm(VmScope::Machine).filter(|vm| vm.editing)?;
        Some(self.lima_form(vm, VmScope::Machine, VmSection::Machine, screen, cx))
    }

    pub(crate) fn lima_machine_feedback(&self, screen: &ScreenContext<'_>) -> Option<Div> {
        let vm = self.lima_vm(VmScope::Machine)?;
        let (key, color) = vm.feedback.or_else(|| {
            vm.task
                .is_some()
                .then_some((gui::GUI_DESKTOP_WORKING, ColorToken::TextMuted))
        })?;
        Some(settings_text(
            screen,
            Role::Small,
            key.text(screen.locale),
            color,
        ))
    }

    fn lima_power_button(
        &self,
        vm: &VmEditor,
        scope: VmScope,
        locale: Locale,
        icon_only: bool,
        cx: &Context<Self>,
    ) -> Option<Button> {
        let settings = vm.original?;
        let (running, icon, label) = lima_next_action(settings)?;
        let label = label.text(locale);
        let pending_changes = vm
            .draft_changes(settings, cx)
            .is_none_or(|changes| changes != LimaSettingsUpdate::default());
        let button = Button::new(if icon_only {
            "overview-lima-lifecycle"
        } else {
            "change-lima-lifecycle"
        })
        .disabled(
            vm.task.is_some()
                || self.services.is_none()
                || !self.lima_provider_matches(scope, cx)
                || (running && pending_changes),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.request_lima_control(scope, VmWork::Lifecycle(running), locale, window, cx);
        }));
        let button = if icon_only {
            button
                .ghost()
                .w(px(36.))
                .h(px(36.))
                .p_0()
                .child(Icon::default().path(icon))
                .accessibility_label(label)
                .tooltip(label)
        } else {
            button.icon(Icon::default().path(icon)).label(label)
        };
        Some(if icon_only {
            button
        } else if running && matches!(scope, VmScope::Machine) {
            button.primary()
        } else {
            button.outline()
        })
    }

    fn lima_vm(&self, scope: VmScope) -> Option<&VmEditor> {
        match scope {
            VmScope::Editor => self
                .environment_settings
                .editor
                .as_ref()
                .map(|editor| &editor.vm),
            VmScope::Machine => self.machine_vm.as_ref().filter(|vm| {
                matches!(
                    self.route,
                    crate::desktop::Route::Machine | crate::desktop::Route::Overview
                ) && self.machine_lima_target.is_some()
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
        if !self.lima_provider_matches(scope, cx)
            || (matches!(scope, VmScope::Machine) && window.has_active_dialog(cx))
        {
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
                            if matches!(work, VmWork::Edit(..)) {
                                vm.editing = false;
                            }
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
        let mut body = div().flex().flex_col().gap_4();
        if !matches!(section, VmSection::Machine) {
            let actions = row(locale)
                .gap_2()
                .flex_wrap()
                .children(self.lima_power_button(vm, scope, locale, false, cx))
                .child(
                    Button::new("refresh-lima-settings")
                        .outline()
                        .icon(IconName::RefreshCw)
                        .tooltip(gui::GUI_DESKTOP_REFRESH.text(locale))
                        .accessibility_label(gui::GUI_DESKTOP_REFRESH.text(locale))
                        .disabled(busy || self.services.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_lima_vm(scope, VmWork::Inspect, window, cx);
                        })),
                );
            body = body
                .child(
                    row(locale)
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .flex_wrap()
                        .child(settings_text(
                            screen,
                            Role::Lead,
                            match section {
                                VmSection::Network => gui::GUI_ENVIRONMENT_LIMA_NETWORK,
                                _ => gui::GUI_ENVIRONMENT_LIMA_RESOURCES,
                            }
                            .text(locale),
                            ColorToken::TextPrimary,
                        ))
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
        }
        let Some(settings) = vm.original else {
            return body;
        };
        let disabled = busy || !settings.stopped || !provider_matches;
        if !matches!(section, VmSection::Machine) {
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
        }
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

pub(super) fn lima_next_action(settings: LimaSettings) -> Option<(bool, &'static str, TextKey)> {
    if settings.running && !settings.stopped {
        Some((
            false,
            "icons/square-stop.svg",
            gui::GUI_ENVIRONMENT_LIMA_LIFECYCLE_STOP,
        ))
    } else if settings.stopped && !settings.running {
        Some((
            true,
            "icons/play.svg",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lima_power_control_requires_a_verified_running_or_stopped_state() {
        let stopped = LimaSettings {
            backend: "Virtualization.framework",
            architecture: Some("aarch64"),
            cpus: 2,
            memory_bytes: 2 * GIB,
            disk_bytes: 12 * GIB,
            stopped: true,
            running: false,
            host_loopback: None,
        };
        assert_eq!(
            lima_next_action(stopped).map(|(start, _, _)| start),
            Some(true)
        );
        assert_eq!(
            lima_next_action(LimaSettings {
                stopped: false,
                running: true,
                ..stopped
            })
            .map(|(start, _, _)| start),
            Some(false),
        );
        for (stopped_state, running) in [(false, false), (true, true)] {
            assert!(
                lima_next_action(LimaSettings {
                    stopped: stopped_state,
                    running,
                    ..stopped
                })
                .is_none()
            );
        }
    }
}
