//! Host machine, settings, storage and activity screens.

mod activity;
mod approvals;
mod disk;
mod machine;
mod notifications;
mod overview;
mod settings;

use marsa_ui::gui_keys as gui;
use std::{
    collections::{BTreeSet, VecDeque},
    future::Future,
    sync::Arc,
};

use marsa_client::{
    ClientError,
    api::{
        AuditDecision, DaemonSnapshot, DiskUsageResponse, EngineState, GetSettingsRequest,
        ListAgentAuditResponse, PrunePreviewResponse, SettingsSnapshot, VersionResponse,
        VmStatsSample,
    },
};
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, EventEmitter, ParentElement, Styled, Task, Window, div},
    gpui_component::button::{Button, ButtonVariants},
    preferences::{AccentColor, Language, Settings, Theme},
};

use crate::{
    desktop::Route,
    screens::common::{ScreenContext, align, color, row},
    services::{Pending, Services},
};

pub(crate) use notifications::{NotificationDestination, Notifications, notification_destination};
use settings::{ResourceInputs, SettingsTab};

pub(crate) struct OpenRoute(pub(crate) Route);

impl EventEmitter<OpenRoute> for SystemView {}

pub(crate) struct SystemView {
    services: Option<Arc<Services>>,
    route: Route,
    settings: Option<(Settings, u64)>,
    restart_required: bool,
    settings_tab: SettingsTab,
    update_panel: settings::UpdatePanel,
    external_inventory: Option<Arc<crate::screens::external_inventory::ExternalWorkspace>>,
    external_inventory_task: Option<Task<()>>,
    external_inventory_checked: bool,
    resource_inputs: ResourceInputs,
    version: Option<VersionResponse>,
    login: Option<marsa_client::login::LoginStatus>,
    disk: Option<DiskUsageResponse>,
    vm_stats: Option<VmStatsSample>,
    vm_stats_task: Option<Task<()>>,
    preview: Option<PrunePreviewResponse>,
    cleanup_selection: BTreeSet<(i32, String)>,
    disk_engine_ready: bool,
    reclaimed: Option<u64>,
    audit: Option<ListAgentAuditResponse>,
    audit_filter: Option<AuditDecision>,
    audit_cursor: Option<String>,
    notice: Option<Notice>,
    command: Option<Task<()>>,
    settings_load: Option<Task<()>>,
    route_load: Option<Task<()>>,
    revision: Option<u64>,
}

#[derive(Clone, Copy)]
enum Notice {
    Working,
    Failed,
    InvalidSettings,
    Saved,
    AppliedNextStart,
    DiagnosticsSaved,
    ProxyPreviousPolicy,
    ProxyRecoveryRequired,
}

impl SystemView {
    pub(crate) fn external_inventory(
        &self,
    ) -> Option<&Arc<crate::screens::external_inventory::ExternalWorkspace>> {
        self.external_inventory.as_ref()
    }

    pub(crate) fn new(
        services: Option<Arc<Services>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            services,
            route: Route::Overview,
            settings: None,
            restart_required: false,
            settings_tab: SettingsTab::General,
            update_panel: settings::UpdatePanel::default(),
            external_inventory: None,
            external_inventory_task: None,
            external_inventory_checked: false,
            resource_inputs: ResourceInputs::new(window, cx),
            version: None,
            login: None,
            disk: None,
            vm_stats: None,
            vm_stats_task: None,
            preview: None,
            cleanup_selection: BTreeSet::new(),
            disk_engine_ready: false,
            reclaimed: None,
            audit: None,
            audit_filter: None,
            audit_cursor: None,
            notice: None,
            command: None,
            settings_load: None,
            route_load: None,
            revision: None,
        }
    }

    pub(crate) fn preferences(&self) -> Option<(Language, Theme)> {
        self.settings
            .as_ref()
            .map(|(settings, _)| (settings.ui.language, settings.ui.theme))
    }

    pub(crate) fn accent(&self) -> AccentColor {
        self.settings
            .as_ref()
            .map_or(AccentColor::MarsaBlue, |(settings, _)| {
                settings.ui.accent_color
            })
    }

    pub(crate) fn prepare_notification_destination(
        &mut self,
        target: NotificationDestination,
    ) -> Route {
        match target {
            NotificationDestination::Machine => Route::Machine,
            NotificationDestination::Activity => Route::Activity,
            NotificationDestination::Updates => {
                self.settings_tab = SettingsTab::Updates;
                Route::Settings
            }
            NotificationDestination::Network => {
                self.settings_tab = SettingsTab::Network;
                Route::Settings
            }
        }
    }

    pub(crate) fn observe(&mut self, snapshot: Option<&DaemonSnapshot>, cx: &mut Context<Self>) {
        let revision = snapshot.map(|snapshot| snapshot.settings_revision);
        let ready = snapshot
            .and_then(|snapshot| snapshot.engine.as_ref())
            .is_some_and(|engine| matches!(engine.state(), EngineState::Idle | EngineState::Ready));
        let disk_became_ready = ready && !self.disk_engine_ready;
        self.disk_engine_ready = ready;
        if !ready {
            self.preview = None;
            self.cleanup_selection.clear();
            self.vm_stats = None;
            self.vm_stats_task = None;
        } else if matches!(self.route, Route::Overview | Route::Machine) {
            self.watch_vm(cx);
        }
        if revision != self.revision {
            self.revision = revision;
            self.settings_load = None;
            if revision.is_some() {
                self.fetch_settings(cx);
                self.enter(self.route, cx);
            }
        } else if disk_became_ready && self.route == Route::Disk {
            self.load_disk(cx);
        }
    }

    pub(crate) fn enter(&mut self, route: Route, cx: &mut Context<Self>) {
        self.route = route;
        self.route_load = None;
        if !matches!(route, Route::Overview | Route::Machine) {
            self.vm_stats_task = None;
            self.vm_stats = None;
        }
        match route {
            Route::Overview | Route::Machine => {
                self.load_disk(cx);
                self.watch_vm(cx);
            }
            Route::Disk => self.load_disk(cx),
            Route::Settings if self.settings_tab == SettingsTab::Updates => {
                self.load_update_status(cx)
            }
            Route::Settings if self.settings_tab == SettingsTab::About => self.load_version(cx),
            Route::Settings if self.settings_tab == SettingsTab::General => self.load_login(cx),
            Route::Activity => self.load_audit(cx),
            _ => {}
        }
    }

    fn watch_vm(&mut self, cx: &mut Context<Self>) {
        if self.vm_stats_task.is_some() {
            return;
        }
        let Some(services) = &self.services else {
            return;
        };
        let mut stream = services.watch_vm();
        self.vm_stats_task = Some(cx.spawn(async move |this, cx| {
            while let Some(sample) = stream.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.vm_stats = Some(sample);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.vm_stats = None;
                this.vm_stats_task = None;
                cx.notify();
            });
        }));
    }

    fn fetch_settings(&mut self, cx: &Context<Self>) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        let mut pending = services.request(|client| async move {
            Ok(client
                .settings()
                .get(GetSettingsRequest {})
                .await?
                .into_inner())
        });
        self.settings_load = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Ok(snapshot)) = result {
                    this.accept_settings(snapshot);
                }
                cx.notify();
            });
        }));
    }

    fn accept_settings(&mut self, snapshot: SettingsSnapshot) {
        match Settings::from_toml(&snapshot.settings_toml) {
            Ok(settings) => {
                self.settings = Some((settings, snapshot.revision));
                self.restart_required = snapshot.restart_required;
            }
            Err(_) => self.notice = Some(Notice::InvalidSettings),
        }
    }

    fn action<T: Send + 'static>(
        &mut self,
        mut pending: Pending<T>,
        finish: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(Notice::Working);
        self.command = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                match result {
                    Some(Ok(result)) => finish(this, result, cx),
                    Some(Err(_)) | None => this.notice = Some(Notice::Failed),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn request<T, F>(
        &mut self,
        operation: impl FnOnce(marsa_client::MarsaClient) -> F + Send + 'static,
        finish: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        T: Send + 'static,
        F: Future<Output = Result<T, ClientError>> + Send + 'static,
    {
        if self.busy() {
            return;
        }
        if let Some(services) = &self.services {
            self.action(services.request(operation), finish, cx);
        }
    }

    fn busy(&self) -> bool {
        matches!(self.notice, Some(Notice::Working))
    }

    fn mutation_id(&mut self, cx: &mut Context<Self>) -> Option<String> {
        match marsa_client::request_id() {
            Ok(id) => Some(id),
            Err(_) => {
                self.notice = Some(Notice::Failed);
                cx.notify();
                None
            }
        }
    }

    pub(crate) fn render_route(
        &self,
        route: Route,
        screen: &ScreenContext<'_>,
        history: &VecDeque<EngineState>,
        cx: &mut Context<Self>,
    ) -> Div {
        let view = match route {
            Route::Overview => self.overview(screen, history, cx),
            Route::Machine => self.machine(screen, cx),
            Route::Disk => self.disk(screen, cx),
            Route::Activity => self.activity(screen, history, cx),
            Route::Settings => self.settings(screen, cx),
            _ => div(),
        };
        align(view, screen.locale)
    }

    pub(crate) fn banner(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Option<Div> {
        let locale = screen.locale;
        let notice = self.notice;
        let message = match notice {
            Some(Notice::Working) => gui::GUI_DESKTOP_WORKING.text(locale),
            Some(Notice::Failed) => {
                gui::GUI_DESKTOP_THE_REQUEST_FAILED_CHECK_THE_ENGINE_STATE_AND_TRY_AGAIN
                    .text(locale)
            }
            Some(Notice::InvalidSettings) => {
                gui::GUI_DESKTOP_MARSA_COULD_NOT_READ_OR_VALIDATE_THESE_SETTINGS_REVIEW_THE_VAL
                    .text(locale)
            }
            Some(Notice::Saved) => gui::GUI_DESKTOP_SETTINGS_SAVED.text(locale),
            Some(Notice::AppliedNextStart) => {
                gui::GUI_DESKTOP_SETTINGS_SAVED_RESOURCE_CHANGES_APPLY_AT_THE_NEXT_START
                    .text(locale)
            }
            Some(Notice::ProxyPreviousPolicy) => {
                gui::GUI_DESKTOP_PROXY_PREVIOUS_POLICY.text(locale)
            }
            Some(Notice::ProxyRecoveryRequired) => {
                gui::GUI_DESKTOP_PROXY_RECOVERY_REQUIRED.text(locale)
            }
            Some(Notice::DiagnosticsSaved) => {
                gui::GUI_DESKTOP_THE_DIAGNOSTICS_BUNDLE_WAS_CREATED_REVIEW_IT_IN_FINDER_BEFORE
                    .text(locale)
            }
            None if screen.state.daemon().is_none() => {
                gui::GUI_DESKTOP_CONNECTING_TO_THE_LOCAL_DAEMON_IF_THE_CONNECTION_STAYS_UNAVAIL
                    .text(locale)
            }
            None if screen
                .state
                .engine()
                .is_some_and(|engine| engine.state() == EngineState::Failed) =>
            {
                gui::GUI_DESKTOP_NOTIFICATION_ENGINE_BODY.text(locale)
            }
            _ => return None,
        };
        let background = match notice {
            Some(Notice::Failed | Notice::ProxyRecoveryRequired) => ColorToken::StatusErrorBg,
            Some(Notice::InvalidSettings | Notice::ProxyPreviousPolicy) => {
                ColorToken::StatusWarningBg
            }
            None if screen
                .state
                .engine()
                .is_some_and(|engine| engine.state() == EngineState::Failed) =>
            {
                ColorToken::StatusErrorBg
            }
            _ => ColorToken::AccentSubtle,
        };
        let dismissible = notice.is_some() && !self.busy();
        Some(
            row(locale)
                .justify_between()
                .gap_2()
                .p_2()
                .rounded_md()
                .bg(color(screen.palette, background))
                .child(align(div().text_sm(), locale).child(message))
                .children(dismissible.then(|| {
                    Button::new("dismiss-notice")
                        .ghost()
                        .label(gui::GUI_DESKTOP_DISMISS.text(locale))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        }))
                })),
        )
    }
}
