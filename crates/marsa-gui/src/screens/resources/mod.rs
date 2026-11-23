//! Image, volume, network, and Compose project routes.

mod compose;
mod forms;
mod logs;
mod operations;
mod pull;
mod rows;

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{collections::BTreeMap, sync::Arc};

use marsa_client::docker::{ExternalEndpoint, ImageDetails, VolumeUsage};

use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        App, AppContext, Context, Div, Entity, IntoElement, ParentElement, Styled, Task, Window,
        div, px, svg,
    },
    gpui_component::{
        Disableable, IconName, WindowExt,
        button::{Button, ButtonVariants},
        input::InputState,
    },
    tokens::{
        scale::{Radius, Space},
        typography::Role,
    },
};

use crate::{
    screens::{
        common::{ScreenContext, align, resource_input, row, technical},
        external_inventory::{Environment, ExternalWorkspace},
    },
    services::Services,
};

use forms::ResourceRequest;
use forms::{Form, FormKind};
use operations::Outcome;
use rows::{Columns, ResourceRow, table};

/// Resource pages reached from the desktop navigator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResourceKind {
    Images,
    Volumes,
    Networks,
    Compose,
}

impl ResourceKind {
    fn icon(self) -> &'static str {
        match self {
            Self::Images => "icons/layers.svg",
            Self::Volumes => "icons/hard-drive.svg",
            Self::Networks => "icons/network.svg",
            Self::Compose => "icons/container.svg",
        }
    }

    fn main_action(self) -> Option<FormKind> {
        match self {
            Self::Images => Some(FormKind::Pull),
            Self::Volumes => Some(FormKind::CreateVolume),
            Self::Networks => Some(FormKind::CreateNetwork),
            Self::Compose => Some(FormKind::DeployStack),
        }
    }
}

/// Persistent controls shared by the resource routes.
pub(crate) struct ResourcesView {
    filter: Entity<InputState>,
    selection: Option<(ResourceKind, String)>,
    selected_endpoint: Option<ExternalEndpoint>,
    reveal_selection: bool,
    form: Option<Form>,
    logs: Option<logs::ProjectLogs>,
    inspect: Option<ImageDetails>,
    inspection_failed: bool,
    inspection_task: Option<Task<()>>,
    busy: Option<ResourceKind>,
    outcome: Option<(ResourceKind, Outcome)>,
    pull: Option<pull::PullState>,
    operation: Option<Task<()>>,
    compose_output: Option<compose::ComposeOutputPane>,
    volume_usage: BTreeMap<String, VolumeUsage>,
    external_volume_usage: Vec<(ExternalEndpoint, BTreeMap<String, VolumeUsage>)>,
    volume_scan: Option<Task<()>>,
    volume_scanning: bool,
    volume_scan_failed: bool,
}

#[derive(Clone)]
struct ResourceSource {
    endpoint: ExternalEndpoint,
    badge: marsa_ui::gpui::SharedString,
    key: String,
    state: crate::store::AppState,
}

impl From<&ExternalWorkspace> for ResourceSource {
    fn from(workspace: &ExternalWorkspace) -> Self {
        Self {
            endpoint: workspace.endpoint.clone(),
            badge: technical(&workspace.source),
            key: format!("{}:{}", workspace.source, workspace.endpoint.address()),
            state: workspace.state.clone(),
        }
    }
}

pub(crate) struct ExternalResourcesChanged;
impl marsa_ui::gpui::EventEmitter<ExternalResourcesChanged> for ResourcesView {}

impl ResourcesView {
    /// Create the content state on the desktop window.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(
            &filter,
            |_, _, _: &marsa_ui::gpui_component::input::InputEvent, cx| cx.refresh_windows(),
        )
        .detach();
        Self {
            filter,
            selection: None,
            selected_endpoint: None,
            reveal_selection: false,
            form: None,
            logs: None,
            inspect: None,
            inspection_failed: false,
            inspection_task: None,
            busy: None,
            outcome: None,
            pull: None,
            operation: None,
            compose_output: None,
            volume_usage: BTreeMap::new(),
            external_volume_usage: Vec::new(),
            volume_scan: None,
            volume_scanning: false,
            volume_scan_failed: false,
        }
    }

    pub(crate) fn set_locale(&self, locale: Locale, window: &mut Window, cx: &mut App) {
        super::common::update_input_placeholder(
            &self.filter,
            gui::GUI_DESKTOP_FILTER_RESOURCES.text(locale),
            window,
            cx,
        );
    }

    /// Reveal the resource chosen from global search.
    ///
    /// An image content ID selects the first row tagged with that image. A
    /// full row key selects a particular tag.
    pub(crate) fn show_resource(
        &mut self,
        kind: ResourceKind,
        key: String,
        endpoint: Option<ExternalEndpoint>,
        cx: &mut Context<Self>,
    ) {
        self.selection = Some((kind, key));
        self.selected_endpoint = endpoint;
        self.reveal_selection = true;
        self.form = None;
        self.logs = None;
        self.inspect = None;
        self.inspection_failed = false;
        self.inspection_task = None;
        cx.refresh_windows();
    }

    /// Stop viewing resource details when leaving the route.
    pub(crate) fn leave(&mut self) {
        self.selection = None;
        self.selected_endpoint = None;
        self.reveal_selection = false;
        self.form = None;
        self.logs = None;
        self.inspect = None;
        self.inspection_task = None;
        self.volume_scan = None;
        self.volume_scanning = false;
    }

    fn open_form(
        &mut self,
        action: (FormKind, Option<ResourceSource>),
        locale: Locale,
        palette: Palette,
        service: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kind, source) = action;
        let title = kind.title(locale);
        let state = source.as_ref().map_or_else(
            || service.store().select(Clone::clone).current().clone(),
            |source| source.state.clone(),
        );
        self.form = Some(Form::new(kind, source, locale, window, cx));
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let screen = ScreenContext {
                locale,
                palette,
                state: &state,
                services: Some(&service),
            };
            let body = view
                .read(cx)
                .form
                .as_ref()
                .map(|form| form.render(&screen, &view, cx));
            dialog
                .width(px(612.))
                .overlay_closable(false)
                .title(text(&screen, Role::Section, title))
                .children(body)
        });
    }
}

/// Render the selected resource page from the current application snapshot.
pub(crate) fn render(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    view: &Entity<ResourcesView>,
    external: &[Arc<ExternalWorkspace>],
    environment: &Entity<Environment>,
    cx: &App,
) -> Div {
    let data = view.read(cx);
    let filter = data.filter.read(cx).value().to_lowercase();
    let context = environment.read(cx);
    let selected_workspaces = external
        .iter()
        .filter(|workspace| context.includes_external(workspace))
        .collect::<Vec<_>>();
    let active_source = selected_workspaces
        .iter()
        .find(|workspace| workspace.reachable)
        .map(|workspace| ResourceSource::from(workspace.as_ref()));
    let available = context.includes_marsa() || active_source.is_some();
    let mut toolbar = logical_row(screen.locale)
        .items_center()
        .gap(px(Space::S8.pixels()))
        .flex_wrap()
        .child(
            div().w(px(216.)).child(
                resource_input(&data.filter)
                    .cleanable(true)
                    .prefix(IconName::Search)
                    .aria_label(message(screen.locale, gui::GUI_DESKTOP_FILTER_RESOURCES)),
            ),
        );
    if let Some(action) = kind.main_action() {
        toolbar = toolbar.child(primary_action(
            screen,
            view,
            action,
            active_source.clone(),
            available,
        ));
    }
    if kind == ResourceKind::Images {
        toolbar = toolbar.child(primary_action(
            screen,
            view,
            FormKind::ImportImage,
            active_source.clone(),
            available,
        ));
    }

    if kind == ResourceKind::Volumes {
        let service = screen.services.cloned();
        let view = view.clone();
        let endpoint = active_source.map(|source| source.endpoint);
        toolbar = toolbar.child(
            Button::new("measure-volume-usage")
                .outline()
                .h(px(Space::S36.pixels()))
                .label(message(
                    screen.locale,
                    gui::GUI_DESKTOP_MEASURE_VOLUME_SIZES,
                ))
                .disabled(service.is_none() || data.volume_scanning || !available)
                .loading(data.volume_scanning)
                .on_click(move |_, _, cx| {
                    if let (true, Some(service)) = (available, &service) {
                        view.update(cx, |view, cx| {
                            view.measure_volumes(service, endpoint.clone(), cx)
                        });
                    }
                }),
        );
    }
    let columns = Columns::for_kind(screen, kind);
    let mut resources = if context.includes_marsa() {
        rows::snapshot_rows(screen, kind, &data.volume_usage)
    } else {
        Vec::new()
    };
    let no_usage = BTreeMap::new();
    for external in &selected_workspaces {
        let foreign_screen = ScreenContext {
            state: &external.state,
            services: None,
            ..*screen
        };
        let usage = data
            .external_volume_usage
            .iter()
            .find(|(endpoint, _)| endpoint == &external.endpoint)
            .map_or(&no_usage, |(_, usage)| usage);
        resources.extend(
            rows::snapshot_rows(&foreign_screen, kind, usage)
                .into_iter()
                .map(|row| row.with_source(external.as_ref().into())),
        );
    }
    let selected = data
        .selection
        .as_ref()
        .filter(|(page, _)| *page == kind)
        .and_then(|(_, key)| {
            resources
                .iter()
                .find(|row| row.selected(key) && row.endpoint() == data.selected_endpoint.as_ref())
        });
    let rows = resources
        .iter()
        .filter(|row| row.matches(&filter))
        .collect::<Vec<_>>();
    let resource_toolbar = logical_row(screen.locale)
        .justify_between()
        .flex_wrap()
        .min_h(px(Space::S48.pixels()))
        .px(px(Space::S8.pixels()))
        .py(px(Space::S4.pixels()))
        .gap(px(Space::S8.pixels()))
        .rounded(px(Radius::R12.pixels()))
        .border_1()
        .border_color(tone(screen, ColorToken::GlassEdge))
        .bg(tone(screen, ColorToken::GlassRail))
        .child(toolbar)
        .children(
            (!resources.is_empty() || screen.state.engine().is_some()).then(|| {
                text(screen, Role::Small, rows.len().to_string())
                    .text_color(tone(screen, ColorToken::TextMuted))
            }),
        );
    let mut content = div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(Space::S16.pixels()))
        .child(resource_toolbar);
    if selected_workspaces.iter().any(|engine| {
        !engine.reachable
            || match kind {
                ResourceKind::Volumes => !engine.volumes_available,
                ResourceKind::Networks => !engine.networks_available,
                _ => false,
            }
    }) {
        content = content.child(text(
            screen,
            Role::Small,
            message(
                screen.locale,
                gui::GUI_DESKTOP_EXTERNAL_RESOURCE_UNAVAILABLE,
            ),
        ));
    }
    content = content.children(selected.map(|row| details(screen, kind, row, view, data)));
    content = content.child(table(
        screen,
        kind,
        &columns,
        &rows,
        view,
        selected.map(|row| row.key()),
        !external.is_empty(),
    ));
    if kind == ResourceKind::Images {
        content = content.children(pull::render(
            screen,
            view,
            data.pull.as_ref(),
            data.busy == Some(ResourceKind::Images),
        ));
    }
    if kind == ResourceKind::Compose {
        content = content.children(compose::render_output(screen, data));
    }
    if kind == ResourceKind::Volumes && data.volume_scan_failed {
        content = content.child(
            text(
                screen,
                Role::Small,
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_DOCKER_DID_NOT_REPORT_VOLUME_SIZES,
                ),
            )
            .text_color(tone(screen, ColorToken::StatusWarning)),
        );
    }
    if let Some(row) = selected
        && kind == ResourceKind::Compose
    {
        content = content.child(logs::render(screen, row, view, data.logs.as_ref()));
    }
    content
}

fn primary_action(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    action: FormKind,
    source: Option<ResourceSource>,
    available: bool,
) -> impl marsa_ui::gpui::IntoElement {
    let state = view.clone();
    let locale = screen.locale;
    let palette = screen.palette;
    let service = screen.services.cloned();
    let title = action.title(locale);
    let button = Button::new(marsa_ui::gpui::SharedString::from(format!(
        "primary-resource-action:{title}"
    )))
    .primary()
    .h(px(Space::S36.pixels()))
    .rounded(px(Radius::R8.pixels()))
    .label(title)
    .disabled(service.is_none() || !available);
    button
        .on_click(move |_, window, cx| {
            if let (true, Some(service)) = (available, &service) {
                state.update(cx, |state, cx| {
                    state.open_form(
                        (action.clone(), source.clone()),
                        locale,
                        palette,
                        Arc::clone(service),
                        window,
                        cx,
                    )
                });
            }
        })
        .into_any_element()
}

fn details(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    row: &ResourceRow,
    view: &Entity<ResourcesView>,
    data: &ResourcesView,
) -> Div {
    let locale = screen.locale;
    let actions = row
        .actions(kind)
        .into_iter()
        .enumerate()
        .map(|(index, action)| {
            let view = view.clone();
            let palette = screen.palette;
            let service = screen.services.cloned();
            let source = row.source().cloned();
            Button::new(("resource-detail-action", index))
                .label(action.title(locale))
                .disabled(screen.services.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(service) = &service {
                        view.update(cx, |view, cx| {
                            view.open_form(
                                (action.clone(), source.clone()),
                                locale,
                                palette,
                                Arc::clone(service),
                                window,
                                cx,
                            );
                        });
                    }
                })
        });
    let mut detail = crate::screens::common::panel(screen)
        .gap(px(Space::S12.pixels()))
        .child(
            logical_row(locale)
                .gap(px(Space::S12.pixels()))
                .child(resource_icon(screen, kind))
                .children(row.source().map(|source| {
                    crate::screens::external_inventory::source_badge(
                        screen.palette,
                        source.badge.clone(),
                    )
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(text(screen, Role::Lead, row.display_name().to_owned()).truncate())
                        .child(
                            text(screen, Role::MonoSmall, row.id.clone())
                                .text_color(tone(screen, ColorToken::TextMuted))
                                .truncate(),
                        ),
                ),
        )
        .children(row.details.iter().cloned().map(|(name, value)| {
            logical_row(locale)
                .justify_between()
                .gap(px(Space::S12.pixels()))
                .py(px(Space::S8.pixels()))
                .border_t_1()
                .border_color(tone(screen, ColorToken::BorderSubtle))
                .child(
                    text(screen, Role::Label, name).text_color(tone(screen, ColorToken::TextMuted)),
                )
                .child(text(screen, Role::Mono, value).truncate())
        }))
        .child(
            logical_row(locale)
                .gap(px(Space::S8.pixels()))
                .flex_wrap()
                .children(actions),
        );
    if kind == ResourceKind::Compose
        && let Some(source) = row.source()
    {
        let controls = [
            (
                marsa_client::docker::ContainerAction::Start,
                "Start project",
                "ابدأ المشروع",
            ),
            (
                marsa_client::docker::ContainerAction::Stop,
                "Stop project",
                "أوقف المشروع",
            ),
            (
                marsa_client::docker::ContainerAction::Restart,
                "Restart project",
                "أعد تشغيل المشروع",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (action, en, ar))| {
            let service = screen.services.cloned();
            let endpoint = source.endpoint.clone();
            let members = row
                .members
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let view = view.clone();
            Button::new(("external-compose-action", index))
                .label(crate::screens::common::label(screen, en, ar))
                .disabled(service.is_none() || data.busy.is_some() || members.is_empty())
                .on_click(move |_, _, cx| {
                    if let Some(service) = &service {
                        view.update(cx, |view, cx| {
                            view.external_project_action(
                                service,
                                endpoint.clone(),
                                members.clone(),
                                action,
                                cx,
                            )
                        });
                    }
                })
        });
        detail = detail.child(
            logical_row(locale)
                .gap(px(Space::S8.pixels()))
                .flex_wrap()
                .children(controls),
        );
    }
    if kind == ResourceKind::Images {
        detail = detail.children(operations::image_metadata(screen, row, data));
    }
    detail
}

fn resource_icon(screen: &ScreenContext<'_>, kind: ResourceKind) -> Div {
    let tint = match kind {
        ResourceKind::Images | ResourceKind::Compose => ColorToken::AccentDefault,
        ResourceKind::Volumes => ColorToken::StatusWarning,
        ResourceKind::Networks => ColorToken::StatusRunning,
    };
    div()
        .flex()
        .size(px(Space::S32.pixels()))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(Radius::R8.pixels()))
        .bg(tone(screen, ColorToken::BgSubtle))
        .child(
            svg()
                .path(kind.icon())
                .size(px(Space::S16.pixels()))
                .text_color(tone(screen, tint)),
        )
}

fn logical_row(locale: Locale) -> Div {
    row(locale)
}

fn text(
    screen: &ScreenContext<'_>,
    role: Role,
    value: impl Into<marsa_ui::gpui::SharedString>,
) -> Div {
    let style = if screen.locale == Locale::Arabic {
        role.arabic()
    } else {
        role.latin()
    };
    let value = value.into();
    let value = if matches!(role, Role::Mono | Role::MonoSmall) {
        technical(value.as_ref())
    } else {
        value
    };
    align(style.apply(div()), screen.locale)
        .child(value)
        .text_color(tone(screen, ColorToken::TextPrimary))
}

fn tone(screen: &ScreenContext<'_>, token: ColorToken) -> marsa_ui::gpui::Hsla {
    screen.palette.color(token).into()
}
