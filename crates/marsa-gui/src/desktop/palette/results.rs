//! Command palette actions and bounded entries from the current resource snapshot.

use marsa_client::{api::EngineState, docker::ContainerId};
use marsa_ui::{
    Locale,
    gpui::{Context, Window},
    gui_keys as gui,
};

use crate::{screens::resources::ResourceKind, services::EngineAction};

use super::super::{Desktop, Route};

const RESULT_LIMIT: usize = 40;

#[derive(Clone)]
pub(super) enum Destination {
    Navigate(Route),
    Engine(EngineAction),
    Diagnostics,
    Setup,
    Container(ContainerId),
    Resource(ResourceKind, String),
}

impl Destination {
    pub(super) fn run(self, desktop: &mut Desktop, window: &mut Window, cx: &mut Context<Desktop>) {
        desktop.close_palette(window, cx);
        match self {
            Self::Navigate(route) => desktop.navigate(route, cx),
            Self::Engine(action) => desktop.request_engine(action, window, cx),
            Self::Diagnostics => {
                desktop.navigate(Route::Machine, cx);
                desktop
                    .system
                    .update(cx, |system, cx| system.collect_diagnostics(cx));
            }
            Self::Setup => desktop.begin_setup(cx),
            Self::Container(id) => {
                desktop.navigate(Route::Containers, cx);
                desktop
                    .containers
                    .update(cx, |view, cx| view.show_container(id, cx));
            }
            Self::Resource(kind, key) => {
                desktop.navigate(resource_route(kind), cx);
                desktop
                    .resources
                    .update(cx, |view, cx| view.show_resource(kind, key, cx));
            }
        }
    }
}

pub(super) struct Entry {
    pub(super) title: String,
    pub(super) detail: Option<&'static str>,
    pub(super) shortcut: Option<&'static str>,
    pub(super) icon: &'static str,
    pub(super) destination: Destination,
}

pub(super) fn search(desktop: &Desktop, value: &str) -> Vec<Entry> {
    let locale = desktop.locale;
    let value = value.trim().to_lowercase();
    let mut actions = vec![
        route_action(Route::Containers, locale),
        route_action(Route::Disk, locale),
        route_action(Route::Settings, locale),
    ];
    if matches!(
        desktop.engine(),
        Some(EngineState::Ready | EngineState::Idle | EngineState::Paused)
    ) {
        actions.push(action(
            gui::GUI_PALETTE_RESTART.text(locale),
            "icons/rotate-cw.svg",
            Destination::Engine(EngineAction::Restart),
        ));
    } else if matches!(
        desktop.engine(),
        Some(EngineState::Stopped | EngineState::Failed)
    ) {
        actions.push(action(
            gui::GUI_PALETTE_START.text(locale),
            "icons/play.svg",
            Destination::Engine(EngineAction::Start),
        ));
    }
    if desktop.state.daemon().is_some() {
        actions.push(action(
            gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS.text(locale),
            "icons/download.svg",
            Destination::Diagnostics,
        ));
    }

    if value.is_empty() {
        return actions;
    }

    if matches!(
        desktop.engine(),
        Some(EngineState::Ready | EngineState::Idle | EngineState::Paused)
    ) {
        actions.push(action(
            gui::GUI_PALETTE_STOP.text(locale),
            "icons/square.svg",
            Destination::Engine(EngineAction::Stop),
        ));
    }
    actions.push(action(
        gui::GUI_SETUP_TITLE.text(locale),
        "icons/settings.svg",
        Destination::Setup,
    ));
    actions.extend(
        Route::WORKSPACE
            .into_iter()
            .chain(Route::SYSTEM)
            .filter(|route| !matches!(route, Route::Containers | Route::Disk | Route::Settings))
            .map(|route| route_action(route, locale)),
    );
    actions.retain(|entry| {
        entry.title.to_lowercase().contains(&value)
            || action_title(&entry.destination, Locale::EnglishUs)
                .to_lowercase()
                .contains(&value)
            || action_title(&entry.destination, Locale::Arabic)
                .to_lowercase()
                .contains(&value)
    });
    append_resources(desktop, &value, &mut actions);
    actions
}

fn action(title: &str, icon: &'static str, destination: Destination) -> Entry {
    Entry {
        title: title.into(),
        detail: None,
        shortcut: None,
        icon,
        destination,
    }
}

fn route_action(route: Route, locale: Locale) -> Entry {
    let destination = Destination::Navigate(route);
    let title = action_title(&destination, locale);
    let mut entry = action(&title, route.icon(), destination);
    entry.shortcut = match route {
        Route::Containers => Some("⌘ 1"),
        Route::Disk => Some("⌘ 6"),
        Route::Settings => Some("⌘ ,"),
        _ => None,
    };
    entry
}

fn action_title(destination: &Destination, locale: Locale) -> String {
    let text = match destination {
        Destination::Navigate(Route::Containers) => gui::GUI_DESKTOP_OPEN_CONTAINERS.text(locale),
        Destination::Navigate(Route::Disk) => gui::GUI_PALETTE_OPEN_DISK.text(locale),
        Destination::Navigate(Route::Settings) => gui::GUI_PALETTE_OPEN_SETTINGS.text(locale),
        Destination::Navigate(route) => return route.name(locale).into(),
        Destination::Engine(EngineAction::Start) => gui::GUI_PALETTE_START.text(locale),
        Destination::Engine(EngineAction::Stop) => gui::GUI_PALETTE_STOP.text(locale),
        Destination::Engine(EngineAction::Restart) => gui::GUI_PALETTE_RESTART.text(locale),
        Destination::Diagnostics => gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS.text(locale),
        Destination::Setup => gui::GUI_SETUP_TITLE.text(locale),
        _ => "",
    };
    text.into()
}

fn append_resources(desktop: &Desktop, value: &str, entries: &mut Vec<Entry>) {
    let locale = desktop.locale;
    let state = &desktop.state;

    fn contains(text: &str, value: &str) -> bool {
        text.to_lowercase().contains(value)
    }

    fn resource(title: &str, route: Route, locale: Locale, destination: Destination) -> Entry {
        let mut entry = action(title, route.icon(), destination);
        entry.detail = Some(route.name(locale));
        entry
    }

    let containers = state.containers();
    let images = state.images();
    let volumes = state.volumes();
    let networks = state.networks();
    let projects = state.compose_projects();
    let exact_matches = containers
        .values()
        .filter_map(|container| {
            let id = container.id.as_str();
            let name = container
                .names
                .first()
                .map_or(id, |name| name.trim_start_matches('/'));
            (contains(name, value) || contains(id, value)).then(|| {
                resource(
                    name,
                    Route::Containers,
                    locale,
                    Destination::Container(container.id.clone()),
                )
            })
        })
        .chain(images.values().filter_map(|image| {
            let tag = image.repo_tags.iter().find(|tag| contains(tag, value));
            if tag.is_none() && !contains(&image.id, value) {
                return None;
            }
            let mut key = image.id.clone();
            let name = if let Some(tag) = tag {
                key.push(':');
                key.push_str(tag);
                tag.as_str()
            } else {
                image.repo_tags.first().unwrap_or(&image.id)
            };
            Some(resource(
                name,
                Route::Images,
                locale,
                Destination::Resource(ResourceKind::Images, key),
            ))
        }))
        .chain(volumes.values().filter_map(|volume| {
            contains(&volume.name, value).then(|| {
                resource(
                    &volume.name,
                    Route::Volumes,
                    locale,
                    Destination::Resource(ResourceKind::Volumes, volume.name.clone()),
                )
            })
        }))
        .chain(networks.values().filter_map(|network| {
            (contains(&network.name, value) || contains(&network.id, value)).then(|| {
                resource(
                    &network.name,
                    Route::Networks,
                    locale,
                    Destination::Resource(ResourceKind::Networks, network.id.clone()),
                )
            })
        }))
        .chain(projects.values().filter_map(|project| {
            contains(&project.name, value).then(|| {
                resource(
                    &project.name,
                    Route::Compose,
                    locale,
                    Destination::Resource(ResourceKind::Compose, project.name.clone()),
                )
            })
        }));

    entries.extend(exact_matches.take(RESULT_LIMIT.saturating_sub(entries.len())));
}

fn resource_route(kind: ResourceKind) -> Route {
    match kind {
        ResourceKind::Images => Route::Images,
        ResourceKind::Volumes => Route::Volumes,
        ResourceKind::Networks => Route::Networks,
        ResourceKind::Compose => Route::Compose,
    }
}
