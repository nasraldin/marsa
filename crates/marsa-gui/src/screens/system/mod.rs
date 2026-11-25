//! Host machine, settings, storage and activity screens.

mod activity;
mod approvals;
mod disk;
mod machine;
mod notifications;
mod settings;
pub(crate) use settings::{SettingsTab, environment_icon_path};

use marsa_ui::gui_keys as gui;
use std::{
    collections::{BTreeSet, VecDeque},
    future::Future,
    sync::Arc,
    time::Duration,
};

use marsa_client::{
    ClientError,
    api::{
        AuditDecision, DaemonSnapshot, DiskUsageResponse, EngineState, GetSettingsRequest,
        ListAgentAuditResponse, PrunePreviewResponse, SettingsSnapshot, VersionResponse,
        VmStatsSample,
    },
    release::AboutAssets,
};
use marsa_ui::{
    Locale,
    gpui::{AnyElement, Context, Div, EventEmitter, Task, Window, div},
    preferences::{AccentColor, DisplaySettings, Language, Settings, Theme},
};

use crate::{
    desktop::{
        Route,
        toast::{self, Tone},
    },
    screens::common::{ScreenContext, align},
    services::{Pending, Services},
};

pub(crate) use notifications::{NotificationDestination, Notifications, notification_destination};
use settings::{EnvironmentSettings, GitSources, RegistrySettings, ResourceInputs, VmEditor};

pub(crate) struct OpenRoute(pub(crate) Route);

pub(crate) struct SelectContext(pub(crate) crate::screens::external_inventory::Environment);

impl EventEmitter<SelectContext> for SystemView {}

impl EventEmitter<OpenRoute> for SystemView {}

pub(crate) struct SystemView {
    services: Option<Arc<Services>>,
    route: Route,
    settings: Option<(Settings, u64)>,
    restart_required: bool,
    settings_tab: SettingsTab,
    update_panel: settings::UpdatePanel,
    external_inventory: crate::screens::external_inventory::Workspaces,
    external_inventory_task: Option<Task<()>>,
    environment_settings: EnvironmentSettings,
    machine_vm: Option<VmEditor>,
    machine_lima_target: Option<marsa_client::docker::LimaInstance>,
    environment_observations: settings::Observations,
    git_sources: GitSources,
    registry_settings: RegistrySettings,
    resource_inputs: ResourceInputs,
    version: Option<VersionResponse>,
    about_assets: Option<AboutAssets>,
    about_assets_task: Option<Task<()>>,
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
    presented_notice: Option<Notice>,
    daemon_notice: Option<Notice>,
    daemon_notice_dismissed: bool,
    notice_timeout: Option<Task<()>>,
    command: Option<Task<()>>,
    settings_load: Option<Task<()>>,
    route_load: Option<Task<()>>,
    revision: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Notice {
    Working,
    Failed,
    InvalidSettings,
    Saved,
    AppliedNextStart,
    DiagnosticsSaved,
    ProxyPreviousPolicy,
    ProxyRecoveryRequired,
    DaemonUnavailable,
    EngineFailed,
}

impl SystemView {
    pub(crate) fn external_inventory_pending(&self) -> bool {
        self.external_inventory_task.is_some()
    }

    pub(crate) fn external_inventory(&self) -> &crate::screens::external_inventory::Workspaces {
        &self.external_inventory
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
            external_inventory: Arc::from([]),
            external_inventory_task: None,
            environment_settings: EnvironmentSettings::default(),
            machine_vm: None,
            machine_lima_target: None,
            environment_observations: settings::Observations::default(),
            git_sources: GitSources::default(),
            registry_settings: RegistrySettings::default(),
            resource_inputs: ResourceInputs::new(window, cx),
            version: None,
            about_assets: None,
            about_assets_task: None,
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
            presented_notice: None,
            daemon_notice: None,
            daemon_notice_dismissed: false,
            notice_timeout: None,
            command: None,
            settings_load: None,
            route_load: None,
            revision: None,
        }
    }

    pub(crate) fn config_sets(
        &self,
    ) -> &[marsa_client::docker::container_config::ContainerConfigSet] {
        self.settings
            .as_ref()
            .map(|(settings, _)| settings.ui.container_config_sets.as_slice())
            .unwrap_or_default()
    }

    pub(crate) fn preferences(&self) -> Option<(Language, Theme)> {
        self.settings
            .as_ref()
            .map(|(settings, _)| (settings.ui.language, settings.ui.theme))
    }

    pub(crate) fn log_settings(&self) -> marsa_ui::preferences::LogSettings {
        self.settings
            .as_ref()
            .map(|(settings, _)| settings.ui.logs)
            .unwrap_or_default()
    }

    pub(crate) fn display_settings(&self) -> DisplaySettings {
        self.settings
            .as_ref()
            .map_or_else(DisplaySettings::default, |(settings, _)| {
                settings.ui.display
            })
    }

    pub(crate) fn accent(&self) -> AccentColor {
        self.settings
            .as_ref()
            .map_or(AccentColor::MarsaBlue, |(settings, _)| {
                settings.ui.accent_color
            })
    }

    /// Return the saved Compose template, or the built-in one if settings are available.
    pub(crate) fn compose_template(&self) -> Option<&str> {
        let (settings, _) = self.settings.as_ref()?;
        Some(
            settings
                .ui
                .compose_template
                .as_deref()
                .unwrap_or(marsa_client::compose::DEFAULT_TEMPLATE),
        )
    }

    /// Select the Docker settings tab and update the detected engine list.
    pub(crate) fn select_docker_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_tab = SettingsTab::Environments;
        self.refresh_environments(cx);
        cx.notify();
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
                self.settings_tab = SettingsTab::Environments;
                Route::Settings
            }
        }
    }

    pub(crate) fn observe(&mut self, snapshot: Option<&DaemonSnapshot>, cx: &mut Context<Self>) {
        if snapshot.is_none() {
            self.version = None;
        }
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
        } else if self.route == Route::Machine {
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
        if self.route != route {
            self.clear_lima_machine_target();
        }
        self.route = route;
        self.route_load = None;
        if route != Route::Machine {
            self.vm_stats_task = None;
            self.vm_stats = None;
        }
        match route {
            Route::Machine => {
                self.load_disk(cx);
                self.watch_vm(cx);
            }
            Route::Disk => self.load_disk(cx),
            Route::Settings if self.settings_tab == SettingsTab::Updates => {
                self.load_update_status(cx)
            }
            Route::Settings if self.settings_tab == SettingsTab::About => self.load_about(cx),
            Route::Settings if self.settings_tab == SettingsTab::GitSources => {
                self.load_git_sources(cx)
            }
            Route::Settings if self.settings_tab == SettingsTab::Registries => {
                self.load_registries(cx)
            }
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
        compact_settings: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let view = match route {
            Route::Machine => self.machine(screen, cx),
            Route::Disk => self.disk(screen, cx),
            Route::Activity => self.activity(screen, history, cx),
            Route::Settings => self.settings(screen, compact_settings, cx),
            _ => div(),
        };
        align(view, screen.locale)
    }

    /// Present operation feedback above the workspace content.
    pub(crate) fn toast(
        &mut self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let daemon_notice = if screen.state.daemon().is_none() {
            Some(Notice::DaemonUnavailable)
        } else if screen
            .state
            .engine()
            .is_some_and(|engine| engine.state() == EngineState::Failed)
        {
            Some(Notice::EngineFailed)
        } else {
            None
        };
        if self.daemon_notice != daemon_notice {
            self.daemon_notice = daemon_notice;
            self.daemon_notice_dismissed = false;
        }

        let operation_notice = self.notice;
        let notice = operation_notice.or(daemon_notice.filter(|_| !self.daemon_notice_dismissed));
        if self.presented_notice != notice {
            self.presented_notice = notice;
            self.notice_timeout = notice.filter(|notice| notice.transient()).map(|notice| {
                let timer = cx.background_executor().timer(Duration::from_secs(6));
                cx.spawn(async move |view, cx| {
                    timer.await;
                    let _ = view.update(cx, |this, cx| {
                        if this.notice == Some(notice) {
                            this.notice = None;
                            cx.notify();
                        }
                    });
                })
            });
        }

        let notice = notice?;
        let locale = screen.locale;
        let (message, tone) = notice.presentation(locale);
        let dismiss = toast::dismiss_button("dismiss-notice", locale, screen.palette).on_click(
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if operation_notice.is_none() && this.daemon_notice == Some(notice) {
                    this.daemon_notice_dismissed = true;
                } else if this.notice == operation_notice {
                    this.notice = None;
                }
                cx.notify();
            }),
        );
        Some(toast::render(
            ("system-toast", notice as usize),
            locale,
            screen.palette,
            tone,
            message,
            Some(dismiss),
            cx,
        ))
    }
}

impl Notice {
    fn presentation(self, locale: Locale) -> (&'static str, Tone) {
        let (message, tone) = match self {
            Self::Working => (gui::GUI_DESKTOP_WORKING, Tone::Info),
            Self::Failed => (
                gui::GUI_DESKTOP_THE_REQUEST_FAILED_CHECK_THE_ENGINE_STATE_AND_TRY_AGAIN,
                Tone::Error,
            ),
            Self::InvalidSettings => (
                gui::GUI_DESKTOP_MARSA_COULD_NOT_READ_OR_VALIDATE_THESE_SETTINGS_REVIEW_THE_VAL,
                Tone::Warning,
            ),
            Self::Saved => (gui::GUI_DESKTOP_SETTINGS_SAVED, Tone::Success),
            Self::AppliedNextStart => (
                gui::GUI_DESKTOP_SETTINGS_SAVED_RESOURCE_CHANGES_APPLY_AT_THE_NEXT_START,
                Tone::Success,
            ),
            Self::ProxyPreviousPolicy => (gui::GUI_DESKTOP_PROXY_PREVIOUS_POLICY, Tone::Warning),
            Self::ProxyRecoveryRequired => (gui::GUI_DESKTOP_PROXY_RECOVERY_REQUIRED, Tone::Error),
            Self::DiagnosticsSaved => (
                gui::GUI_DESKTOP_THE_DIAGNOSTICS_BUNDLE_WAS_CREATED_REVIEW_IT_IN_FINDER_BEFORE,
                Tone::Success,
            ),
            Self::DaemonUnavailable => (
                gui::GUI_DESKTOP_CONNECTING_TO_THE_LOCAL_DAEMON_IF_THE_CONNECTION_STAYS_UNAVAIL,
                Tone::Info,
            ),
            Self::EngineFailed => (gui::GUI_DESKTOP_NOTIFICATION_ENGINE_BODY, Tone::Error),
        };
        (message.text(locale), tone)
    }

    fn transient(self) -> bool {
        matches!(
            self,
            Self::Saved | Self::AppliedNextStart | Self::DiagnosticsSaved
        )
    }
}
