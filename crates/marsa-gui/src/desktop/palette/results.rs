//! Command palette actions and bounded entries from the current resource snapshot.

use marsa_client::{
    api::EngineState,
    docker::{ContainerId, ExternalEndpoint},
};
use marsa_ui::{
    Locale,
    gpui::{App, Context, Window},
    gui_keys as gui,
};

use crate::{
    screens::{resources::ResourceKind, system::SettingsTab},
    services::EngineAction,
};

use super::super::{Desktop, Route};

const RESULT_LIMIT: usize = 40;

#[derive(Clone)]
pub(super) enum Destination {
    Navigate(Route),
    Settings(SettingsTab),
    Engine(EngineAction),
    Diagnostics,
    Setup,
    Container(ContainerId, Option<ExternalEndpoint>),
    Resource(ResourceKind, String, Option<ExternalEndpoint>),
}

impl Destination {
    pub(super) fn run(self, desktop: &mut Desktop, window: &mut Window, cx: &mut Context<Desktop>) {
        desktop.close_palette(window, cx);
        match self {
            Self::Navigate(route) => desktop.navigate(route, cx),
            Self::Settings(section) => {
                desktop.navigate(Route::Settings, cx);
                desktop
                    .system
                    .update(cx, |system, cx| system.select_settings_tab(section, cx));
            }
            Self::Engine(action) => desktop.request_engine(action, window, cx),
            Self::Diagnostics => {
                desktop.navigate(Route::Machine, cx);
                desktop
                    .system
                    .update(cx, |system, cx| system.collect_diagnostics(cx));
            }
            Self::Setup => desktop.begin_setup(cx),
            Self::Container(id, endpoint) => {
                desktop.navigate(Route::Containers, cx);
                desktop
                    .containers
                    .update(cx, |view, cx| view.show_container(id, endpoint, cx));
            }
            Self::Resource(kind, key, endpoint) => {
                desktop.navigate(resource_route(kind), cx);
                desktop
                    .resources
                    .update(cx, |view, cx| view.show_resource(kind, key, endpoint, cx));
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

pub(super) fn search(desktop: &Desktop, value: &str, cx: &App) -> Vec<Entry> {
    let locale = desktop.locale;
    let local = desktop.environment.read(cx).includes_marsa();
    let engine = local.then(|| desktop.engine()).flatten();
    let value = value.trim().to_lowercase();
    let mut actions = vec![
        route_action(Route::Containers, locale),
        route_action(Route::Disk, locale),
        route_action(Route::Settings, locale),
    ];
    if matches!(
        engine,
        Some(EngineState::Ready | EngineState::Idle | EngineState::Paused)
    ) {
        actions.push(action(
            gui::GUI_PALETTE_RESTART.text(locale),
            "icons/rotate-cw.svg",
            Destination::Engine(EngineAction::Restart),
        ));
    } else if matches!(engine, Some(EngineState::Stopped | EngineState::Failed)) {
        actions.push(action(
            gui::GUI_PALETTE_START.text(locale),
            "icons/play.svg",
            Destination::Engine(EngineAction::Start),
        ));
    }
    if local && desktop.state.daemon().is_some() {
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
        engine,
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
    actions.extend(
        SettingsTab::GROUPS
            .into_iter()
            .flat_map(|(_, sections)| sections.iter().copied())
            .map(|section| {
                let destination = Destination::Settings(section);
                action(
                    &action_title(&destination, locale),
                    section.icon(),
                    destination,
                )
            }),
    );
    actions.retain(|entry| {
        entry.title.to_lowercase().contains(&value)
            || action_title(&entry.destination, Locale::EnglishUs)
                .to_lowercase()
                .contains(&value)
            || action_title(&entry.destination, Locale::Arabic)
                .to_lowercase()
                .contains(&value)
            || (matches!(
                entry.destination,
                Destination::Settings(SettingsTab::General)
            ) && [
                gui::GUI_SETTINGS_TYPOGRAPHY_SECTION,
                gui::GUI_SETTINGS_UI_FONT,
                gui::GUI_SETTINGS_UI_FONT_SIZE,
                gui::GUI_SETTINGS_TERMINAL_FONT,
                gui::GUI_SETTINGS_TIME_FORMAT,
                gui::GUI_DESKTOP_SETTINGS_APPEARANCE,
                gui::GUI_DESKTOP_SETTINGS_LANGUAGE,
            ]
            .into_iter()
            .any(|key| {
                [Locale::EnglishUs, Locale::Arabic]
                    .into_iter()
                    .any(|locale| key.text(locale).to_lowercase().contains(&value))
            }))
    });
    append_resources(desktop, &value, &mut actions, cx);
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
        Destination::Settings(section) => {
            return format!(
                "{} · {}",
                Route::Settings.name(locale),
                section.title(locale)
            );
        }
        Destination::Engine(EngineAction::Start) => gui::GUI_PALETTE_START.text(locale),
        Destination::Engine(EngineAction::Stop) => gui::GUI_PALETTE_STOP.text(locale),
        Destination::Engine(EngineAction::Restart) => gui::GUI_PALETTE_RESTART.text(locale),
        Destination::Diagnostics => gui::GUI_DESKTOP_COLLECT_DIAGNOSTICS.text(locale),
        Destination::Setup => gui::GUI_SETUP_TITLE.text(locale),
        _ => "",
    };
    text.into()
}

fn append_resources(desktop: &Desktop, value: &str, entries: &mut Vec<Entry>, cx: &App) {
    let locale = desktop.locale;
    let (state, endpoint) = if desktop.environment.read(cx).includes_marsa() {
        (&desktop.state, None)
    } else if let Some(workspace) = desktop.active_workspace(cx) {
        (&workspace.state, Some(workspace.endpoint.clone()))
    } else {
        return;
    };

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
                    Destination::Container(container.id.clone(), endpoint.clone()),
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
                Destination::Resource(ResourceKind::Images, key, endpoint.clone()),
            ))
        }))
        .chain(
            volumes
                .values()
                .filter(|volume| contains(&volume.name, value))
                .map(|volume| {
                    resource(
                        &volume.name,
                        Route::Volumes,
                        locale,
                        Destination::Resource(
                            ResourceKind::Volumes,
                            volume.name.clone(),
                            endpoint.clone(),
                        ),
                    )
                }),
        )
        .chain(
            networks
                .values()
                .filter(|network| contains(&network.name, value) || contains(&network.id, value))
                .map(|network| {
                    resource(
                        &network.name,
                        Route::Networks,
                        locale,
                        Destination::Resource(
                            ResourceKind::Networks,
                            network.id.clone(),
                            endpoint.clone(),
                        ),
                    )
                }),
        )
        .chain(
            projects
                .values()
                .filter(|project| contains(&project.name, value))
                .map(|project| {
                    resource(
                        &project.name,
                        Route::Compose,
                        locale,
                        Destination::Resource(
                            ResourceKind::Compose,
                            project.name.clone(),
                            endpoint.clone(),
                        ),
                    )
                }),
        );

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
