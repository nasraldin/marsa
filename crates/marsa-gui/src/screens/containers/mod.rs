//! Containers, their lifecycle commands, and selected-resource streams.

mod ansi;
mod create;
mod detail;
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
    RevealedEnvironment,
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

pub(crate) struct ContainerView {
    services: Option<Arc<Services>>,
    locale: Locale,
    palette: Palette,
    state: AppState,
    input: Option<Entity<InputState>>,
    query: String,
    filter: StatusFilter,
    selected: BTreeSet<ContainerId>,
    focused: Option<ContainerId>,
    tab: Tab,
    descending: bool,
    mode: list::ViewMode,
    collapsed: BTreeSet<Option<String>>,
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
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|view, cx| view.terminal.close_detached(cx))
            .detach();
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
            services,
            locale,
            palette,
            state,
            input: None,
            query: String::new(),
            filter: StatusFilter::All,
            selected: BTreeSet::new(),
            focused: None,
            tab: Tab::Overview,
            descending: false,
            mode: list::ViewMode::default(),
            collapsed: BTreeSet::new(),
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
        cx: &mut Context<Self>,
    ) {
        self.locale = locale;
        self.palette = palette;
        cx.notify();
    }

    pub(crate) fn leave(&mut self, cx: &mut Context<Self>) {
        self.close_detail(cx);
    }

    fn replace_snapshot(&mut self, state: AppState, cx: &mut Context<Self>) {
        self.selected.retain(|id| state.container(id).is_some());
        if self
            .focused
            .as_ref()
            .is_some_and(|id| state.container(id).is_none())
        {
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
    pub(crate) fn show_container(&mut self, id: ContainerId, cx: &mut Context<Self>) {
        if self.state.container(&id).is_some() {
            self.open_detail(id, cx);
        }
    }

    fn open_detail(&mut self, id: ContainerId, cx: &mut Context<Self>) {
        self.focused = Some(id);
        self.combined_logs = false;
        self.set_tab(Tab::Overview, cx);
    }

    fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.focused = None;
        self.set_tab(Tab::Overview, cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.stream_tasks.clear();
        self.terminal.close_detached(cx);
        self.terminal = terminal::State::default();
        self.revealed = None;
        self.reveal_task = None;
        self.reveal_failed = false;
        self.logs = logs::LogsState::default();
        self.stats = stats::StatsState::default();
        self.inspection = Inspection::Idle;
        self.tab = tab;
        let Some(id) = self.focused.clone() else {
            cx.notify();
            return;
        };
        let Some(services) = self.services.as_ref() else {
            cx.notify();
            return;
        };
        match tab {
            Tab::Logs => {
                let selected = self.state.container(&id);
                let project = selected.and_then(ContainerSummary::compose_project);
                let containers = self.state.containers();
                if let Some(project) = project.filter(|_| self.combined_logs) {
                    let matches = containers
                        .values()
                        .filter(|entry| entry.compose_project() == Some(project));
                    self.logs
                        .set_source_limit(matches.clone().count() > logs::MAX_SOURCES);
                    for entry in matches.take(logs::MAX_SOURCES) {
                        self.stream_tasks.push(logs::follow(
                            services,
                            entry.id.clone(),
                            Arc::from(name(entry)),
                            cx,
                        ));
                    }
                } else {
                    let label = selected.map_or(id.as_str(), name);
                    self.stream_tasks.push(logs::follow(
                        services,
                        id.clone(),
                        Arc::from(label),
                        cx,
                    ));
                }
                self.logs.set_stream_count(self.stream_tasks.len());
            }
            Tab::Stats => self.stream_tasks.push(stats::follow(services, id, cx)),
            Tab::Overview | Tab::Inspect => {
                self.inspection = Inspection::Loading;
                let mut pending = services.inspect(id);
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
            Tab::Terminal => {}
        }
        cx.notify();
    }

    fn reveal_environment(&mut self, name: &str, cx: &mut Context<Self>) {
        self.revealed = None;
        self.reveal_task = None;
        self.reveal_failed = false;
        let (Some(id), Some(services)) = (self.focused.clone(), self.services.as_ref()) else {
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

    fn request_remove(
        &mut self,
        ids: Vec<ContainerId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        removal::open(self, ids, window, cx);
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

    fn screen(&self) -> ScreenContext<'_> {
        ScreenContext {
            locale: self.locale,
            palette: self.palette,
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
        if let Some(entry) = self
            .focused
            .as_ref()
            .and_then(|id| self.state.container(id))
        {
            detail::render(self, entry, window, cx)
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
    }
}

fn color(palette: Palette, token: ColorToken) -> Hsla {
    palette.color(token).into()
}
