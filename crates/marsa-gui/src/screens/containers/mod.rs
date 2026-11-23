//! Containers, their lifecycle commands, and selected-resource streams.

mod ansi;
mod create;
mod detail;
mod files;
mod list;
mod logs;
mod removal;
mod states;
mod stats;
mod terminal;

use marsa_ui::gui_keys as gui;
use std::{collections::BTreeSet, sync::Arc};

use marsa_client::docker::{
    ContainerAction, ContainerDetails, ContainerId, ContainerSummary, EnvironmentReveal,
    ExternalEndpoint, RevealedEnvironment,
};
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        AppContext as _, Context, Entity, Hsla, IntoElement, Render, Subscription, Task, Window,
    },
    gpui_component::input::{InputEvent, InputState},
};

use crate::{
    screens::common::ScreenContext,
    services::{Command, Services},
    store::AppState,
};

#[derive(Clone)]
struct Snapshot(AppState);

enum Inspection {
    Idle,
    Loading,
    Ready(Box<ContainerDetails>),
    Failed,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.0.engine() == other.0.engine()
            && Arc::ptr_eq(&self.0.containers(), &other.0.containers())
            && Arc::ptr_eq(&self.0.images(), &other.0.images())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Logs,
    Terminal,
    Inspect,
    Stats,
    Files,
}

#[derive(Clone, PartialEq, Eq)]
enum ContainerTarget {
    Marsa(ContainerId),
    External {
        id: ContainerId,
        endpoint: ExternalEndpoint,
    },
}

impl ContainerTarget {
    fn id(&self) -> &ContainerId {
        match self {
            Self::Marsa(id) | Self::External { id, .. } => id,
        }
    }

    fn endpoint(&self) -> Option<&ExternalEndpoint> {
        match self {
            Self::Marsa(_) => None,
            Self::External { endpoint, .. } => Some(endpoint),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum StatusFilter {
    #[default]
    All,
    Running,
    Paused,
    Stopped,
}

impl StatusFilter {
    fn accepts(self, state: &str) -> bool {
        match self {
            Self::All => true,
            Self::Running => state == "running",
            Self::Paused => state == "paused",
            Self::Stopped => !matches!(state, "running" | "paused"),
        }
    }
}

use super::external_inventory::{Environment, ExternalWorkspace, Workspaces};

pub(crate) struct ContainerView {
    services: Option<Arc<Services>>,
    config_sets: Vec<marsa_client::docker::container_config::ContainerConfigSet>,
    locale: Locale,
    palette: Palette,
    display: marsa_ui::preferences::DisplaySettings,
    state: AppState,
    external_inventory: Workspaces,
    file_operation: Option<marsa_ui::gpui::Task<()>>,
    file_result: Option<(ContainerTarget, bool)>,
    environment: marsa_ui::gpui::Entity<Environment>,
    _context_subscription: Subscription,
    input: Option<Entity<InputState>>,
    query: String,
    filter: StatusFilter,
    selected: BTreeSet<ContainerId>,
    focused: Option<ContainerTarget>,
    tab: Tab,
    descending: bool,
    mode: list::ViewMode,
    collapsed: BTreeSet<list::GroupKey>,
    list_scroll: marsa_ui::gpui::UniformListScrollHandle,
    command_pending: bool,
    connection_waited: bool,
    command_failed: bool,
    inspection: Inspection,
    revealed: Option<(String, RevealedEnvironment)>,
    reveal_failed: bool,
    reveal_task: Option<Task<()>>,
    logs: logs::LogsState,
    stats: stats::StatsState,
    terminal: terminal::State,
    _store_task: Option<Task<()>>,
    _connection_timer: Task<()>,
    stream_tasks: Vec<Task<()>>,
    combined_logs: bool,
    input_subscription: Option<Subscription>,
}

impl ContainerView {
    pub(crate) fn new(
        services: Option<Arc<Services>>,
        locale: Locale,
        palette: Palette,
        environment: marsa_ui::gpui::Entity<Environment>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|view, cx| view.terminal.close_detached(cx))
            .detach();
        let _context_subscription = cx.observe(&environment, |view, _, cx| {
            view.selected.clear();
            view.close_detail(cx);
            cx.notify();
        });
        let mut state = AppState::default();
        let store_task = services.as_ref().map(|services| {
            let mut selection = services.store().select(|state| Snapshot(state.clone()));
            state = selection.current().0.clone();
            cx.spawn(async move |view, cx| {
                while let Some(snapshot) = selection.changed().await.cloned() {
                    if view
                        .update(cx, |view, cx| view.replace_snapshot(snapshot.0, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let clock = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(3));
        let connection_timer = cx.spawn(async move |view, cx| {
            clock.await;
            let _ = view.update(cx, |view, cx| {
                view.connection_waited = true;
                cx.notify();
            });
        });
        Self {
            file_operation: None,
            file_result: None,
            services,
            config_sets: Vec::new(),
            locale,
            palette,
            display: Default::default(),
            state,
            input: None,
            query: String::new(),
            external_inventory: Arc::from([]),
            environment,
            _context_subscription,
            filter: StatusFilter::All,
            selected: BTreeSet::new(),
            focused: None,
            tab: Tab::Overview,
            descending: false,
            mode: list::ViewMode::default(),
            collapsed: BTreeSet::new(),
            list_scroll: marsa_ui::gpui::UniformListScrollHandle::new(),
            command_pending: false,
            connection_waited: false,
            command_failed: false,
            inspection: Inspection::Idle,
            revealed: None,
            reveal_failed: false,
            reveal_task: None,
            logs: logs::LogsState::default(),
            stats: stats::StatsState::default(),
            terminal: terminal::State::default(),
            _store_task: store_task,
            _connection_timer: connection_timer,
            stream_tasks: Vec::new(),
            combined_logs: false,
            input_subscription: None,
        }
    }

    pub(crate) fn set_appearance(
        &mut self,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(input) = &self.input {
            super::common::update_input_placeholder(
                input,
                gui::GUI_DESKTOP_SEARCH_CONTAINERS.text(locale),
                window,
                cx,
            );
        }
        if let Some(input) = &self.logs.search_input {
            super::common::update_input_placeholder(
                input,
                gui::GUI_DESKTOP_SEARCH_LOGS.text(locale),
                window,
                cx,
            );
        }
        self.locale = locale;
        self.palette = palette;
        cx.notify();
    }

    pub(crate) fn leave(&mut self, cx: &mut Context<Self>) {
        self.close_detail(cx);
    }

    fn replace_snapshot(&mut self, state: AppState, cx: &mut Context<Self>) {
        self.selected.retain(|id| state.container(id).is_some());
        if self.focused.as_ref().is_some_and(
            |target| matches!(target, ContainerTarget::Marsa(id) if state.container(id).is_none()),
        ) {
            self.close_detail(cx);
        }
        self.state = state;
        cx.notify();
    }

    fn prepare_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.is_some() {
            return;
        }
        let placeholder = gui::GUI_DESKTOP_SEARCH_CONTAINERS.text(self.locale);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        self.input_subscription =
            Some(cx.subscribe(&input, |view, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    view.query = input.read(cx).value().to_lowercase();
                    cx.notify();
                }
            }));
        self.input = Some(input);
    }

    /// Open an existing container from the desktop command palette.
    pub(crate) fn show_container(
        &mut self,
        id: ContainerId,
        endpoint: Option<ExternalEndpoint>,
        cx: &mut Context<Self>,
    ) {
        let target = match endpoint {
            Some(endpoint) => ContainerTarget::External { id, endpoint },
            None => ContainerTarget::Marsa(id),
        };
        self.open_detail(target, cx);
    }

    fn open_detail(&mut self, target: ContainerTarget, cx: &mut Context<Self>) {
        if self
            .target_state(&target)
            .and_then(|state| state.container(target.id()))
            .is_none()
        {
            return;
        }
        self.focused = Some(target);
        self.combined_logs = false;
        self.set_tab(Tab::Overview, cx);
    }

    fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.focused = None;
        self.set_tab(Tab::Overview, cx);
    }

    fn workspace(&self, endpoint: &ExternalEndpoint) -> Option<&ExternalWorkspace> {
        self.external_inventory
            .iter()
            .find(|inventory| inventory.endpoint == *endpoint)
            .map(Arc::as_ref)
    }

    fn target_state(&self, target: &ContainerTarget) -> Option<&AppState> {
        match target {
            ContainerTarget::Marsa(_) => Some(&self.state),
            ContainerTarget::External { endpoint, .. } => {
                self.workspace(endpoint).map(|inventory| &inventory.state)
            }
        }
    }

    pub(crate) fn set_log_settings(
        &mut self,
        settings: marsa_ui::preferences::LogSettings,
        cx: &mut Context<Self>,
    ) {
        self.logs.set_settings(settings, cx);
    }

    pub(crate) fn set_display_settings(
        &mut self,
        settings: marsa_ui::preferences::DisplaySettings,
        cx: &mut Context<Self>,
    ) {
        if self.display != settings {
            self.display = settings;
            cx.notify();
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.stream_tasks.clear();
        self.terminal.close_detached(cx);
        self.terminal = terminal::State::default();
        self.revealed = None;
        self.reveal_task = None;
        self.reveal_failed = false;
        self.logs = logs::LogsState::new(self.logs.settings());
        self.stats = stats::StatsState::default();
        self.inspection = Inspection::Idle;
        self.tab = tab;
        let Some(target) = self.focused.clone() else {
            cx.notify();
            return;
        };
        let Some(services) = self.services.as_ref() else {
            cx.notify();
            return;
        };
        let id = target.id().clone();
        match tab {
            Tab::Logs => {
                let Some(state) = self.target_state(&target) else {
                    return;
                };
                let selected = state.container(&id);
                let project = selected
                    .and_then(ContainerSummary::compose_project)
                    .map(str::to_owned);
                let label: Arc<str> = Arc::from(selected.map_or(id.as_str(), name));
                let containers = state.containers();
                let open_stream = |id| match target.endpoint() {
                    Some(endpoint) => services.external_logs(endpoint.clone(), id),
                    None => services.logs(id),
                };
                if let Some(project) = project.as_deref().filter(|_| self.combined_logs) {
                    let matches = containers
                        .values()
                        .filter(|entry| entry.compose_project() == Some(project));
                    self.logs
                        .set_source_limit(matches.clone().count() > logs::MAX_SOURCES);
                    for entry in matches.take(logs::MAX_SOURCES) {
                        self.stream_tasks.push(logs::follow(
                            open_stream(entry.id.clone()),
                            Arc::from(name(entry)),
                            cx,
                        ));
                    }
                } else {
                    self.stream_tasks
                        .push(logs::follow(open_stream(id), label, cx));
                }
                self.logs.set_stream_count(self.stream_tasks.len());
            }
            Tab::Stats => self.stream_tasks.push(stats::follow(
                services.stats(id, target.endpoint().cloned()),
                cx,
            )),
            Tab::Overview | Tab::Inspect => {
                self.inspection = Inspection::Loading;
                let mut pending = match target.endpoint() {
                    Some(endpoint) => services.inspect_external(endpoint.clone(), id),
                    None => services.inspect(id),
                };
                let task = cx.spawn(async move |view, cx| {
                    let inspection = match pending.result().await {
                        Some(Ok(value)) => Inspection::Ready(Box::new(value)),
                        _ => Inspection::Failed,
                    };
                    let _ = view.update(cx, |view, cx| {
                        view.inspection = inspection;
                        cx.notify();
                    });
                });
                self.stream_tasks.push(task);
            }
            Tab::Terminal | Tab::Files => {}
        }
        cx.notify();
    }

    fn reveal_environment(&mut self, name: &str, cx: &mut Context<Self>) {
        self.revealed = None;
        self.reveal_task = None;
        self.reveal_failed = false;
        let (Some(ContainerTarget::Marsa(id)), Some(services)) =
            (self.focused.clone(), self.services.as_ref())
        else {
            return;
        };
        let Ok(request) = EnvironmentReveal::confirmed(id, name) else {
            self.reveal_failed = true;
            cx.notify();
            return;
        };
        let name = name.to_owned();
        let mut pending = services.reveal_environment(request);
        self.reveal_task = Some(cx.spawn(async move |view, cx| {
            let result = pending.result().await;
            let _ = view.update(cx, |view, cx| {
                if let Some(Ok(Some(value))) = result {
                    view.revealed = Some((name, value));
                } else {
                    view.reveal_failed = true;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn dispatch(
        &mut self,
        mut ids: Vec<ContainerId>,
        action: ContainerAction,
        cx: &mut Context<Self>,
    ) {
        ids.retain(|id| {
            self.state
                .container(id)
                .is_some_and(|entry| eligible(entry, action))
        });
        if let Some(services) = self
            .services
            .clone()
            .filter(|_| !ids.is_empty() && !self.command_pending)
        {
            self.command_failed = false;
            self.command_pending = true;
            let commands = ids
                .into_iter()
                .map(|id| Command::Container { id, action })
                .collect();
            self.run_commands(services, commands, cx);
        }
    }

    fn dispatch_target(
        &mut self,
        target: ContainerTarget,
        action: ContainerAction,
        cx: &mut Context<Self>,
    ) {
        let ContainerTarget::External { id, endpoint } = target else {
            self.dispatch(vec![target.id().clone()], action, cx);
            return;
        };
        if self.command_pending {
            return;
        }
        let Some(inventory) = self.workspace(&endpoint) else {
            return;
        };
        if !inventory
            .state
            .container(&id)
            .is_some_and(|entry| eligible(entry, action))
        {
            return;
        }
        let Some(services) = self.services.as_ref() else {
            return;
        };
        let pending = services.act_external(endpoint.clone(), id.clone(), action);
        self.watch_external(endpoint, id, pending, cx);
    }

    fn watch_external(
        &mut self,
        endpoint: ExternalEndpoint,
        id: ContainerId,
        mut pending: crate::services::Pending<(bool, Option<Vec<ContainerSummary>>)>,
        cx: &mut Context<Self>,
    ) {
        self.command_failed = false;
        self.command_pending = true;
        cx.spawn(async move |view, cx| {
            let (failed, containers) = match pending.result().await {
                Some(Ok(result)) => result,
                _ => (true, None),
            };
            let _ = view.update(cx, |view, cx| {
                if let Some(containers) = containers {
                    view.replace_external_containers(&endpoint, containers);
                }
                view.command_pending = false;
                view.command_failed = failed;
                if matches!(view.tab, Tab::Overview | Tab::Inspect)
                    && view.focused.as_ref().is_some_and(|selected| {
                        selected.id() == &id && selected.endpoint() == Some(&endpoint)
                    })
                {
                    if view
                        .target_state(&ContainerTarget::External {
                            id: id.clone(),
                            endpoint: endpoint.clone(),
                        })
                        .and_then(|state| state.container(&id))
                        .is_some()
                    {
                        view.set_tab(view.tab, cx);
                    } else {
                        view.close_detail(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn replace_external_containers(
        &mut self,
        endpoint: &ExternalEndpoint,
        containers: Vec<ContainerSummary>,
    ) {
        if let Some(inventory) = Arc::make_mut(&mut self.external_inventory)
            .iter_mut()
            .find(|inventory| &inventory.endpoint == endpoint)
        {
            Arc::make_mut(inventory)
                .state
                .replace_containers(containers);
        }
    }

    fn request_remove(
        &mut self,
        ids: Vec<ContainerId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        removal::open(self, ids, None, window, cx);
    }

    fn run_commands(
        &self,
        services: Arc<Services>,
        commands: Vec<Command>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |view, cx| {
            let mut failed = false;
            for command in commands {
                let mut pending = services.dispatch(command);
                failed |= !matches!(pending.result().await, Some(Ok(())));
            }
            let mut refresh = services.refresh();
            let _ = refresh.result().await;
            let _ = view.update(cx, |view, cx| {
                view.command_pending = false;
                view.command_failed = failed;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn config_sets(
        &self,
    ) -> &[marsa_client::docker::container_config::ContainerConfigSet] {
        &self.config_sets
    }

    pub(crate) fn set_config_sets(
        &mut self,
        sets: Vec<marsa_client::docker::container_config::ContainerConfigSet>,
    ) {
        self.config_sets = sets;
    }

    pub(crate) fn set_external_inventory(&mut self, inventory: Workspaces, cx: &mut Context<Self>) {
        self.external_inventory = inventory;
        if self.focused.as_ref().is_some_and(|target| {
            target.endpoint().is_some()
                && self
                    .target_state(target)
                    .and_then(|state| state.container(target.id()))
                    .is_none()
        }) {
            self.close_detail(cx);
        }
        cx.notify();
    }

    pub(crate) fn toast(&self, cx: &mut Context<Self>) -> Option<marsa_ui::gpui::AnyElement> {
        use marsa_ui::gpui::{ParentElement, Styled, px, svg};
        use marsa_ui::gpui_component::button::{Button, ButtonVariants};
        if !self.command_failed {
            return None;
        }
        let label = gui::GUI_DESKTOP_DISMISS.text(self.locale);
        let dismiss = Button::new("dismiss-container-toast")
            .ghost()
            .accessibility_label(label)
            .tooltip(label)
            .child(svg().path("icons/x.svg").size(px(14.)))
            .on_click(cx.listener(|view, _, _, cx| {
                view.command_failed = false;
                cx.notify();
            }));
        Some(crate::desktop::toast::render(
            "container-toast",
            self.locale,
            self.palette,
            crate::desktop::toast::Tone::Error,
            gui::GUI_DESKTOP_THE_CONTAINER_ACTION_COULDN_T_BE_COMPLETED.text(self.locale),
            Some(dismiss),
            cx,
        ))
    }

    fn screen(&self) -> ScreenContext<'_> {
        ScreenContext {
            locale: self.locale,
            palette: self.palette,
            display: self.display,
            state: &self.state,
            services: self.services.as_ref(),
        }
    }
}

impl Render for ContainerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_search(window, cx);
        if self.tab == Tab::Terminal {
            self.terminal.prepare(cx);
        }
        if self.tab == Tab::Logs {
            self.logs.prepare_search(window, self.locale, cx);
        }
        if let Some((target, entry)) = self.focused.as_ref().and_then(|target| {
            self.target_state(target)
                .and_then(|state| state.container(target.id()))
                .map(|entry| (target, entry))
        }) {
            detail::render(self, target, entry, window, cx)
        } else {
            list::render(self, cx)
        }
    }
}

fn eligible(entry: &ContainerSummary, action: ContainerAction) -> bool {
    match action {
        ContainerAction::Start => matches!(entry.state.as_str(), "created" | "exited"),
        ContainerAction::Stop => {
            matches!(entry.state.as_str(), "running" | "paused" | "restarting")
        }
        ContainerAction::Pause => entry.state == "running",
        ContainerAction::Unpause => entry.state == "paused",
        ContainerAction::Restart => matches!(
            entry.state.as_str(),
            "created" | "running" | "exited" | "paused" | "restarting"
        ),
    }
}

fn name(entry: &ContainerSummary) -> &str {
    entry
        .names
        .first()
        .map_or(entry.id.as_str(), |name| name.trim_start_matches('/'))
}

fn state_label(state: &str) -> (&'static str, &'static str) {
    match state {
        "running" => ("Running", "قيد التشغيل"),
        "paused" => ("Paused", "متوقفة مؤقتًا"),
        "created" => ("Created", "أُنشئت"),
        "restarting" => ("Restarting", "تُعاد تهيئتها"),
        "removing" => ("Removing", "تُزال"),
        "exited" | "dead" => ("Stopped", "متوقفة"),
        _ => ("Unknown", "غير معروفة"),
    }
}

fn action_label(action: ContainerAction) -> (&'static str, &'static str) {
    match action {
        ContainerAction::Start => ("Start", "تشغيل"),
        ContainerAction::Stop => ("Stop", "إيقاف"),
        ContainerAction::Restart => ("Restart", "إعادة التشغيل"),
        ContainerAction::Pause => ("Pause", "إيقاف مؤقت"),
        ContainerAction::Unpause => ("Resume", "استئناف"),
    }
}

fn color(palette: Palette, token: ColorToken) -> Hsla {
    palette.color(token).into()
}
