//! Observe only the selected engine while the Overview is visible.

mod usage_history;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use marsa_client::{
    api::EngineState,
    docker::{
        ContainerCounts, ContainerId, ContainerSummary, ContainerUtilization, DiskBreakdown,
        ExternalEndpoint, ImageSummary, OverviewClient, OverviewSample, OverviewUtilization,
        local_image_updates,
    },
};
use marsa_ui::{
    ColorToken,
    direction::align_column_to_end,
    gpui::{
        App, Context, Div, Hsla, IntoElement, ParentElement as _, PathBuilder, Styled as _, Task,
        Window, canvas, div, point, px, relative, svg,
    },
    gpui_component::{
        Disableable, IconName,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    preferences::EnvironmentIcon,
    tokens::typography::Role,
};

use crate::{
    screens::{
        common::{ScreenContext, align, color, disk_bytes, panel, row, technical},
        containers::StatusFilter,
        external_inventory::Environment,
        system::environment_icon_path,
    },
    services::{EngineAction, Services},
    store::AppState,
};

use super::{Desktop, Route, status};
use usage_history::{Metric, UsageHistory};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
const DISK_INTERVAL: Duration = Duration::from_secs(60);
const DISK_RETRY: Duration = Duration::from_secs(15);

const REGISTRY_INTERVAL: Duration = Duration::from_secs(300);

type ImageObservations = (
    Arc<BTreeMap<ContainerId, ContainerSummary>>,
    Arc<BTreeMap<String, ImageSummary>>,
);

fn observed_images(state: &AppState) -> Option<ImageObservations> {
    state
        .has_docker_inventory()
        .then(|| (state.containers(), state.images()))
}

fn current_image_observations(
    target: &Target,
    state: &AppState,
    captured: &ImageObservations,
) -> bool {
    *target == Target::Marsa(state.docker_epoch())
        && observed_images(state).as_ref() == Some(captured)
}

#[derive(Clone, PartialEq, Eq)]
enum Target {
    Marsa(u64),
    External(Box<ExternalEndpoint>),
    Checking(Environment),
}

#[derive(Default)]
pub(super) struct Overview {
    target: Option<Target>,
    check_image_updates: bool,
    task: Option<Task<()>>,
    disk_task: Option<Task<()>>,
    version_task: Option<Task<()>>,
    image_task: Option<Task<()>>,
    registry_task: Option<Task<()>>,
    check_registry_updates: bool,
    registry_inventory: Option<ImageObservations>,
    managed_updates: usize,
    sample: Option<OverviewSample>,
    usage_history: UsageHistory,
    disk: Option<DiskBreakdown>,
    disk_pending: bool,
}

impl Overview {
    pub(super) fn sync_inventory(&mut self, state: &AppState) {
        if self.target != Some(Target::Marsa(state.docker_epoch())) || !state.has_docker_inventory()
        {
            return;
        }
        let mut counts = ContainerCounts::from_rows(state.containers().values());
        counts.updates = self.managed_updates;
        let sample = self.sample.get_or_insert_default();
        if sample.utilization.is_none() {
            sample.utilization = Some(OverviewUtilization {
                running: counts.running,
                usage: Vec::new(),
            });
        }
        sample.containers = Some(counts);
        sample.stacks = Some(state.compose_projects().len());
        sample.images = Some(state.images().len());
        sample.volumes = Some(state.volumes().len());
        sample.networks = Some(state.networks().len());
    }

    fn stop_tasks(&mut self) {
        self.task = None;
        self.disk_task = None;
        self.version_task = None;
        self.image_task = None;
        self.registry_task = None;
    }

    fn publish_updates(&mut self, count: usize, outdated: BTreeSet<String>) {
        self.managed_updates = count;
        let sample = self.sample.get_or_insert_default();
        if let Some(counts) = &mut sample.containers {
            counts.updates = count;
        }
        sample.updates_checked = true;
        sample.outdated_images = outdated;
    }
}

impl Desktop {
    pub(super) fn prepare_overview(&mut self, cx: &mut Context<Self>) {
        if self.route != Route::Overview || self.onboarding.is_some() {
            if self.overview.task.is_some() && self.overview.sample.is_some() {
                self.overview.usage_history.record(None);
            }
            self.overview.stop_tasks();
            return;
        }
        let selection = self.environment.read(cx);
        let target = if selection.includes_marsa() {
            matches!(self.engine(), Some(EngineState::Ready | EngineState::Idle))
                .then_some(Target::Marsa(self.state.docker_epoch()))
        } else {
            Some(self.active_workspace(cx).map_or_else(
                || Target::Checking(selection.clone()),
                |workspace| Target::External(Box::new(workspace.endpoint.clone())),
            ))
        };
        let monitoring = self.system.read(cx).monitoring();
        let check_image_updates = monitoring.check_image_updates;
        let managed = matches!(target, Some(Target::Marsa(_)));
        let check_registry_updates =
            managed && check_image_updates && monitoring.check_managed_registry_updates;
        let checks_changed = self.overview.check_image_updates != check_image_updates
            || self.overview.check_registry_updates != check_registry_updates;
        if self.overview.target != target || (!managed && checks_changed) {
            self.overview = Overview {
                disk_pending: matches!(target, Some(Target::Marsa(_) | Target::External(_))),
                target: target.clone(),
                check_image_updates,
                check_registry_updates,
                ..Overview::default()
            };
        } else if checks_changed {
            self.overview.check_image_updates = check_image_updates;
            self.overview.check_registry_updates = check_registry_updates;
            self.overview.image_task = None;
            self.overview.registry_task = None;
            self.overview.registry_inventory = None;
            self.overview.managed_updates = 0;
            if let Some(sample) = &mut self.overview.sample {
                sample.updates_checked = false;
                sample.outdated_images.clear();
                if let Some(counts) = &mut sample.containers {
                    counts.updates = 0;
                }
            }
            if check_image_updates
                && self.overview.task.is_some()
                && let Some((target, services)) = target.clone().zip(self.services.clone())
            {
                self.observe_managed_image_updates(target, services, cx);
            }
        }
        if self.overview.task.is_some() {
            return;
        }
        self.overview.sync_inventory(&self.state);
        let Some((target, services)) = target.zip(self.services.clone()) else {
            self.overview.disk_pending = false;
            return;
        };
        let executor = cx.background_executor().clone();
        if !matches!(target, Target::Checking(_)) {
            let storage_target = target.clone();
            let storage_services = services.clone();
            let storage_executor = executor.clone();
            self.overview.disk_task = Some(cx.spawn(async move |this, cx| {
                if matches!(&storage_target, Target::Marsa(_)) {
                    let mut request = storage_services.request(|client| async move {
                        Ok(OverviewClient::Marsa(client).disk_cached().await)
                    });
                    if let Some(Ok(Some(cached))) = request.result().await
                        && this
                            .update(cx, |this, cx| {
                                this.publish_overview_disk(&storage_target, Some(cached), cx)
                            })
                            .ok()
                            != Some(true)
                    {
                        return;
                    }
                }
                loop {
                    let disk = match &storage_target {
                        Target::Marsa(_) => {
                            let mut request = storage_services.request(|client| async move {
                                Ok(OverviewClient::Marsa(client).disk().await)
                            });
                            request.result().await.and_then(Result::ok).flatten()
                        }
                        Target::External(endpoint) => {
                            let client = OverviewClient::external(endpoint);
                            let mut request =
                                storage_services.pending(async move { Ok(client.disk().await) });
                            request.result().await.and_then(Result::ok).flatten()
                        }
                        Target::Checking(_) => return,
                    };
                    let interval = if disk.is_some() {
                        DISK_INTERVAL
                    } else {
                        DISK_RETRY
                    };
                    if this
                        .update(cx, |this, cx| {
                            this.publish_overview_disk(&storage_target, disk, cx)
                        })
                        .ok()
                        != Some(true)
                    {
                        return;
                    }
                    storage_executor.timer(interval).await;
                }
            }));
        }
        if matches!(target, Target::Marsa(_)) {
            self.observe_managed_overview(target, services, cx);
            return;
        }
        self.overview.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let sample = match &target {
                    Target::External(endpoint) => {
                        let client = OverviewClient::external(endpoint);
                        let mut request = services.pending(async move {
                            Ok(client.sample_with_image_checks(check_image_updates).await)
                        });
                        request.result().await.and_then(Result::ok)
                    }
                    Target::Checking(_) => None,
                    Target::Marsa(_) => return,
                };
                if this
                    .update(cx, |this, cx| {
                        if this.overview.target.as_ref() != Some(&target) {
                            return false;
                        }
                        let sample = sample.unwrap_or_default();
                        if !observed(&sample) {
                            this.system
                                .update(cx, |system, cx| system.refresh_external_inventory(cx));
                        }
                        this.overview
                            .usage_history
                            .record(sample.utilization.as_ref());
                        this.overview.sample = Some(sample);
                        cx.notify();
                        true
                    })
                    .ok()
                    != Some(true)
                {
                    return;
                }
                executor.timer(SAMPLE_INTERVAL).await;
            }
        }));
    }

    fn observe_managed_overview(
        &mut self,
        target: Target,
        services: Arc<Services>,
        cx: &mut Context<Self>,
    ) {
        let version_services = Arc::clone(&services);
        let version_target = target.clone();
        self.overview.version_task = Some(cx.spawn(async move |this, cx| {
            for prefer_cache in [true, false] {
                let mut request = version_services.request(move |client| async move {
                    Ok(OverviewClient::Marsa(client)
                        .engine_version(prefer_cache)
                        .await)
                });
                if let Some(Ok(Some(version))) = request.result().await
                    && this
                        .update(cx, |desktop, cx| {
                            if desktop.overview.target.as_ref() != Some(&version_target) {
                                return false;
                            }
                            desktop.overview.sample.get_or_insert_default().version = Some(version);
                            cx.notify();
                            true
                        })
                        .ok()
                        != Some(true)
                {
                    return;
                }
            }
        }));

        if self.overview.check_image_updates {
            self.observe_managed_image_updates(target.clone(), Arc::clone(&services), cx);
        }

        let executor = cx.background_executor().clone();
        self.overview.task = Some(cx.spawn(async move |this, cx| {
            let mut loaded = services.store().select(AppState::has_docker_inventory);
            while !loaded.current() {
                if loaded.changed().await.is_none() {
                    return;
                }
            }
            loop {
                let rows = services
                    .store()
                    .select(|state| state.has_docker_inventory().then(|| state.containers()))
                    .current()
                    .clone();
                if let Some(rows) = rows {
                    let mut request = services.request(move |client| async move {
                        Ok(OverviewClient::Marsa(client)
                            .container_utilization(rows.values())
                            .await)
                    });
                    let utilization = request.result().await.and_then(Result::ok);
                    if this
                        .update(cx, |desktop, cx| {
                            let overview = &mut desktop.overview;
                            if overview.target.as_ref() != Some(&target) {
                                return false;
                            }
                            overview.usage_history.record(utilization.as_ref());
                            overview.sample.get_or_insert_default().utilization = utilization;
                            cx.notify();
                            true
                        })
                        .ok()
                        != Some(true)
                    {
                        return;
                    }
                }
                executor.timer(SAMPLE_INTERVAL).await;
            }
        }));
    }

    fn observe_managed_image_updates(
        &mut self,
        target: Target,
        services: Arc<Services>,
        cx: &mut Context<Self>,
    ) {
        if self.overview.check_registry_updates {
            self.observe_managed_registry_updates(target.clone(), Arc::clone(&services), cx);
        }
        self.overview.image_task = Some(cx.spawn(async move |this, cx| {
            let mut selection = services.store().select(observed_images);
            loop {
                if let Some(inputs) = selection.current().clone() {
                    let (containers, images) = inputs.clone();
                    let mut request = services.pending(async move {
                        let containers = containers.values().cloned().collect::<Vec<_>>();
                        let images = images.values().cloned().collect::<Vec<_>>();
                        Ok(local_image_updates(&containers, &images))
                    });
                    if let Some(Ok((count, outdated))) = request.result().await
                        && this
                            .update(cx, |desktop, cx| {
                                let overview = &mut desktop.overview;
                                if overview.target.as_ref() != Some(&target)
                                    || !overview.check_image_updates
                                {
                                    return false;
                                }
                                if current_image_observations(&target, &desktop.state, &inputs)
                                    && overview.registry_inventory.as_ref() != Some(&inputs)
                                {
                                    overview.publish_updates(count, outdated);
                                    cx.notify();
                                }
                                true
                            })
                            .ok()
                            != Some(true)
                    {
                        return;
                    }
                }
                if selection.changed().await.is_none() {
                    return;
                }
            }
        }));
    }

    fn observe_managed_registry_updates(
        &mut self,
        target: Target,
        services: Arc<Services>,
        cx: &mut Context<Self>,
    ) {
        let executor = cx.background_executor().clone();
        self.overview.registry_task = Some(cx.spawn(async move |this, cx| {
            let mut selection = services.store().select(observed_images);
            loop {
                let inputs = loop {
                    if let Some(inputs) = selection.current().clone() {
                        break inputs;
                    }
                    if selection.changed().await.is_none() {
                        return;
                    }
                };
                let (containers, images) = inputs.clone();
                let mut request = services.request(move |client| async move {
                    let containers = containers.values().cloned().collect::<Vec<_>>();
                    let images = images.values().cloned().collect::<Vec<_>>();
                    Ok(OverviewClient::Marsa(client)
                        .registry_image_updates(&containers, &images)
                        .await)
                });
                if let Some(Ok((count, outdated))) = request.result().await
                    && this
                        .update(cx, |desktop, cx| {
                            let overview = &mut desktop.overview;
                            if overview.target.as_ref() != Some(&target)
                                || !overview.check_registry_updates
                            {
                                return false;
                            }
                            if current_image_observations(&target, &desktop.state, &inputs) {
                                overview.registry_inventory = Some(inputs);
                                overview.publish_updates(count, outdated);
                                cx.notify();
                            }
                            true
                        })
                        .ok()
                        != Some(true)
                {
                    return;
                }
                executor.timer(REGISTRY_INTERVAL).await;
                // The store selector's cursor advances only when changed() is awaited.
                selection = services.store().select(observed_images);
            }
        }));
    }

    fn refresh_overview(&mut self, cx: &mut Context<Self>) {
        self.overview.stop_tasks();
        if matches!(self.overview.target, Some(Target::Marsa(_)))
            && let Some(services) = self.services.clone()
        {
            cx.spawn(async move |_, _| {
                let _ = services.refresh().result().await;
            })
            .detach();
        }
    }

    fn publish_overview_disk(
        &mut self,
        target: &Target,
        disk: Option<DiskBreakdown>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.overview.target.as_ref() != Some(target) {
            return false;
        }
        self.overview.disk = disk;
        self.overview.disk_pending = false;
        cx.notify();
        true
    }

    pub(super) fn overview_screen(
        &self,
        screen: &ScreenContext<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let selection = self.environment.read(cx);
        let local = selection.includes_marsa();
        let external = self.active_workspace(cx);
        let data = self.overview.sample.as_ref();
        let engine = if !local
            && data.is_some_and(|data| !observed(data))
            && matches!(self.overview.target, Some(Target::External(_)))
        {
            Some(EngineState::Failed)
        } else {
            self.context_engine(cx)
        };
        let workspace = self
            .external_inventory
            .iter()
            .find(|workspace| selection.includes_external(workspace));
        let (name, glyph) = if local || workspace.is_some() {
            let (name, icon) = self
                .system
                .read(cx)
                .environment_presentation(workspace.map(AsRef::as_ref));
            (technical(&name), icon)
        } else {
            (selection.display(), EnvironmentIcon::Server)
        };
        let version = data.and_then(|data| data.version.as_ref());
        let version_text = version.map(|details| details.version.as_str()).or_else(|| {
            external.and_then(|workspace| {
                workspace
                    .verified_engine
                    .as_ref()
                    .map(|engine| engine.version.as_str())
            })
        });
        let platform = version.map(|details| format!("{}/{}", details.os, details.arch));
        let (lifecycle, lima_state, lima_feedback) = if local {
            let power = overview_power_action(engine).map(|(action, label, icon)| {
                let label = label.text(locale);
                let button = Button::new("overview-engine-lifecycle")
                    .size(px(36.))
                    .p_0()
                    .child(icon)
                    .accessibility_label(label)
                    .tooltip(label)
                    .on_click(cx.listener(move |desktop, _, window, cx| {
                        desktop.request_engine(action, window, cx);
                    }));
                if action == EngineAction::Start {
                    button.primary()
                } else {
                    button.ghost()
                }
            });
            (power, None, None)
        } else {
            self.system.update(cx, |system, cx| {
                (
                    system.overview_lima_power(locale, cx),
                    system.lima_machine_state(),
                    system.lima_machine_feedback(screen),
                )
            })
        };
        let status_label = if lima_state == Some(EngineState::Stopped) {
            gui::GUI_DESKTOP_STOPPED.text(locale)
        } else {
            self.context_status_text(engine, cx)
        };
        let status_state = lima_state
            .filter(|state| *state == EngineState::Stopped)
            .or(engine);
        let ready = matches!(engine, Some(EngineState::Ready | EngineState::Idle));
        if !ready {
            let explanation = match (local, engine) {
                (true, Some(EngineState::Stopped)) => gui::GUI_SYSTEM_MACHINE_STOPPED_HINT,
                (true, Some(EngineState::Failed)) => gui::GUI_DESKTOP_NOTIFICATION_ENGINE_BODY,
                (false, _) if lima_state == Some(EngineState::Stopped) => {
                    gui::GUI_SYSTEM_MACHINE_STOPPED_HINT
                }
                (false, Some(EngineState::Failed)) => gui::GUI_ENVIRONMENT_OFFLINE,
                _ => gui::GUI_DESKTOP_OVERVIEW_WAITING_ENGINE,
            };
            // Same empty state for Marsa, Lima, and any other inactive context.
            return div()
                .flex_1()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_5()
                .px_6()
                .child(
                    div()
                        .size(px(72.))
                        .rounded(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(color(screen.palette, ColorToken::AccentSubtle))
                        .child(
                            svg()
                                .path(environment_icon_path(glyph))
                                .size(px(36.))
                                .text_color(color(screen.palette, ColorToken::AccentDefault)),
                        ),
                )
                .child(
                    screen
                        .typography(Role::Title)
                        .apply(div())
                        .text_center()
                        .child(name),
                )
                .child(status::inline_badge(
                    locale,
                    screen.palette,
                    status_state,
                    status_label,
                ))
                .child(
                    quiet(screen)
                        .max_w(px(420.))
                        .text_center()
                        .child(explanation.text(locale)),
                )
                .child(
                    row(locale)
                        .items_center()
                        .gap_2()
                        .children(lifecycle)
                        .child(
                            Button::new("overview-inactive-refresh")
                                .outline()
                                .icon(IconName::RefreshCw)
                                .label(gui::GUI_DESKTOP_REFRESH.text(locale))
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    desktop.refresh_overview(cx);
                                    if !desktop.environment.read(cx).includes_marsa() {
                                        desktop.system.update(cx, |system, cx| {
                                            system.refresh_lima_selection(window, cx)
                                        });
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("overview-inactive-manage")
                                .ghost()
                                .icon(IconName::Settings2)
                                .label(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
                                .on_click(
                                    cx.listener(|desktop, _, _, cx| desktop.open_contexts(cx)),
                                ),
                        ),
                )
                .children(
                    lima_feedback.map(|feedback| div().max_w(px(480.)).w_full().child(feedback)),
                );
        }
        let refresh = Button::new("overview-refresh")
            .ghost()
            .size(px(36.))
            .p_0()
            .child(IconName::RefreshCw)
            .accessibility_label(gui::GUI_DESKTOP_REFRESH.text(locale))
            .tooltip(gui::GUI_DESKTOP_REFRESH.text(locale))
            .on_click(cx.listener(|desktop, _, window, cx| {
                desktop.refresh_overview(cx);
                if !desktop.environment.read(cx).includes_marsa() {
                    desktop
                        .system
                        .update(cx, |system, cx| system.refresh_lima_selection(window, cx));
                }
                cx.notify();
            }));
        let connection_type = if local {
            Some(gui::GUI_DESKTOP_LOCAL_ENGINE.text(locale))
        } else {
            external.and_then(|workspace| {
                connection_type_label(locale, workspace.endpoint.connection_type()?)
            })
        };
        let actions = row(locale)
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .children(lifecycle)
            .child(refresh)
            .child(
                Button::new("overview-manage-environments")
                    .ghost()
                    .size(px(36.))
                    .p_0()
                    .child(IconName::Settings2)
                    .accessibility_label(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
                    .tooltip(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
                    .on_click(cx.listener(|desktop, _, _, cx| desktop.open_contexts(cx))),
            );
        let identity = panel(screen)
            .child(
                row(locale)
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .size(px(48.))
                            .flex_shrink_0()
                            .rounded(px(12.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(color(screen.palette, ColorToken::AccentSubtle))
                            .child(icon(
                                screen,
                                environment_icon_path(glyph),
                                ColorToken::AccentDefault,
                            )),
                    )
                    .child(
                        align(div().flex_1().min_w_0().flex().flex_col().gap_2(), locale)
                            .child(
                                // Keep stop/refresh/settings on the title row so chips stay one line.
                                row(locale)
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        row(locale)
                                            .gap_3()
                                            .flex_1()
                                            .min_w_0()
                                            .items_center()
                                            .child(
                                                screen
                                                    .typography(Role::Title)
                                                    .apply(div().min_w_0())
                                                    .truncate()
                                                    .child(name),
                                            ),
                                    )
                                    .child(actions),
                            )
                            .child(
                                row(locale)
                                    .gap_2()
                                    .flex_wrap()
                                    .children(
                                        connection_type
                                            .map(|value| identity_chip(screen, value.to_string())),
                                    )
                                    .children(version_text.map(|value| {
                                        identity_chip(
                                            screen,
                                            format!(
                                                "{} {}",
                                                gui::GUI_DESKTOP_OVERVIEW_RUNTIME.text(locale),
                                                value
                                            ),
                                        )
                                    }))
                                    .children(platform.map(|value| identity_chip(screen, value))),
                            ),
                    ),
            )
            .children(lima_feedback)
            .child(context_status_body(
                screen,
                data,
                data.is_none_or(|data| local && data.containers.is_none()),
                status_state,
                status_label,
                cx,
            ));
        let wide = window.viewport_size().width >= px(if self.collapsed { 900. } else { 1100. });
        let loading = data.is_none_or(|data| local && data.containers.is_none())
            && matches!(
                self.overview.target,
                Some(Target::Marsa(_) | Target::External(_))
            );
        // Explicit 2×2 grid so cards never overflow the content column.
        let cards = align(div().flex().flex_col().gap_3().w_full(), locale)
            .child(
                row(locale)
                    .w_full()
                    .items_stretch()
                    .gap_3()
                    .child(identity.flex_1().min_w_0())
                    .child(
                        usage_card(
                            screen,
                            data.and_then(|data| data.utilization.as_ref()),
                            &self.overview.usage_history,
                            loading,
                        )
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .child(
                row(locale)
                    .w_full()
                    .items_stretch()
                    .gap_3()
                    .child(self.disk_card(screen, cx).flex_1().min_w_0())
                    .child(resources_card(screen, data, cx).flex_1().min_w_0()),
            );
        align(div().flex().flex_col().gap_4().w_full().min_w_0(), locale)
            .child(cards)
            .child(top_containers(
                screen,
                data.and_then(|data| data.utilization.as_ref()),
                &self.overview.usage_history,
                loading,
                wide,
                cx,
            ))
    }

    fn open_container_filter(
        &mut self,
        filter: StatusFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outdated = self
            .overview
            .sample
            .as_ref()
            .map(|sample| sample.outdated_images.clone())
            .unwrap_or_default();
        self.navigate(Route::Containers, cx);
        self.containers.update(cx, |view, cx| {
            view.filter_from_overview(filter, outdated, window, cx)
        });
    }

    fn open_overview_container(&mut self, id: ContainerId, cx: &mut Context<Self>) {
        let endpoint = match self.overview.target.as_ref() {
            Some(Target::External(endpoint)) => Some(endpoint.as_ref().clone()),
            _ => None,
        };
        self.navigate(Route::Containers, cx);
        self.containers
            .update(cx, |view, cx| view.show_container(id, endpoint, cx));
    }

    fn disk_card(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let disk = self.overview.disk.as_ref();
        let categories = [
            (
                Route::Images.name(locale),
                disk.and_then(|disk| disk.images),
                color(screen.palette, ColorToken::DiskImages),
            ),
            (
                Route::Containers.name(locale),
                disk.and_then(|disk| disk.containers),
                color(screen.palette, ColorToken::DiskContainers),
            ),
            (
                Route::Volumes.name(locale),
                disk.and_then(|disk| disk.volumes),
                color(screen.palette, ColorToken::DiskVolumes),
            ),
            (
                gui::GUI_DESKTOP_BUILD_CACHE.text(locale),
                disk.and_then(|disk| disk.build_cache),
                color(screen.palette, ColorToken::DiskBuildCache),
            ),
        ];
        let total = categories
            .iter()
            .filter_map(|(_, value, _)| *value)
            .fold(0u64, u64::saturating_add);
        let segments: Vec<(Hsla, f64)> = categories
            .iter()
            .filter_map(|(_, value, tint)| {
                let value = (*value).filter(|value| *value > 0)?;
                (total > 0).then_some((*tint, value as f64 / total as f64))
            })
            .collect();
        panel(screen)
            .min_w_0()
            .overflow_hidden()
            .child(
                row(locale)
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("overview-open-disk")
                            .ghost()
                            .h_auto()
                            .px_0()
                            .py_0()
                            .child(
                                row(locale)
                                    .items_center()
                                    .gap_2()
                                    .child(icon(
                                        screen,
                                        "icons/hard-drive.svg",
                                        ColorToken::TextMuted,
                                    ))
                                    .child(heading(
                                        screen,
                                        gui::GUI_OVERVIEW_DISK_USAGE.text(locale),
                                    )),
                            )
                            .accessibility_label(gui::GUI_OVERVIEW_VIEW_DISK.text(locale))
                            .tooltip(gui::GUI_OVERVIEW_VIEW_DISK.text(locale))
                            .on_click(
                                cx.listener(|desktop, _, _, cx| desktop.navigate(Route::Disk, cx)),
                            ),
                    )
                    .child(
                        screen
                            .typography(Role::Title)
                            .apply(div().flex_shrink_0())
                            .child(if total > 0 {
                                technical(&disk_bytes(locale, total))
                            } else {
                                technical("—")
                            }),
                    ),
            )
            .children(
                self.overview
                    .disk_pending
                    .then(|| quiet(screen).child(gui::GUI_OVERVIEW_MEASURING.text(locale))),
            )
            .children((!self.overview.disk_pending && total == 0).then(|| {
                quiet(screen).child(gui::GUI_SYSTEM_DISK_BREAKDOWN_UNAVAILABLE.text(locale))
            }))
            .children((total > 0).then(|| {
                row(locale)
                    .items_center()
                    .gap_4()
                    .min_w_0()
                    .child(disk_donut(segments))
                    .child(
                        align(div().flex_1().min_w_0().flex().flex_col().gap_3(), locale).children(
                            categories.map(|(label, value, tint)| {
                                row(locale)
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        row(locale)
                                            .items_center()
                                            .gap_2()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .size(px(8.))
                                                    .flex_shrink_0()
                                                    .rounded_full()
                                                    .bg(tint),
                                            )
                                            .child(quiet(screen).truncate().child(label)),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_sm()
                                            .font_weight(marsa_ui::gpui::FontWeight::SEMIBOLD)
                                            .text_color(color(
                                                screen.palette,
                                                ColorToken::TextPrimary,
                                            ))
                                            .child(value.map_or_else(
                                                || technical("—"),
                                                |bytes| technical(&disk_bytes(locale, bytes)),
                                            )),
                                    )
                            }),
                        ),
                    )
            }))
    }
}

fn overview_power_action(
    engine: Option<EngineState>,
) -> Option<(EngineAction, marsa_ui::TextKey, IconName)> {
    match engine {
        Some(EngineState::Stopped | EngineState::Failed) => Some((
            EngineAction::Start,
            gui::GUI_DESKTOP_START_MACHINE,
            IconName::Play,
        )),
        Some(EngineState::Ready | EngineState::Idle | EngineState::Paused) => Some((
            EngineAction::Stop,
            gui::GUI_DESKTOP_STOP_MACHINE,
            IconName::Square,
        )),
        _ => None,
    }
}

fn observed(sample: &OverviewSample) -> bool {
    sample.containers.is_some()
        || sample.version.is_some()
        || sample.images.is_some()
        || sample.volumes.is_some()
        || sample.networks.is_some()
}

fn identity_chip(screen: &ScreenContext<'_>, value: String) -> Div {
    quiet(screen)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(color(screen.palette, ColorToken::BgSubtle))
        .child(technical(&value))
}

fn connection_type_label(locale: marsa_ui::Locale, kind: &str) -> Option<&'static str> {
    Some(match kind {
        "unix" => gui::GUI_ENVIRONMENT_UNIX.text(locale),
        "ssh" => gui::GUI_ENVIRONMENT_SSH.text(locale),
        "tls" => gui::GUI_ENVIRONMENT_TLS.text(locale),
        "hawser-standard" => gui::GUI_ENVIRONMENT_HAWSER_STANDARD.text(locale),
        "hawser-edge" => gui::GUI_ENVIRONMENT_HAWSER_EDGE.text(locale),
        _ => return None,
    })
}

fn context_status_body(
    screen: &ScreenContext<'_>,
    data: Option<&OverviewSample>,
    loading: bool,
    status_state: Option<EngineState>,
    status_label: &'static str,
    cx: &Context<Desktop>,
) -> Div {
    let locale = screen.locale;
    let count = data.and_then(|data| data.containers.as_ref());
    let states = [
        (
            StatusFilter::Running,
            count.map(|count| count.running),
            "icons/play.svg",
            ColorToken::StatusRunning,
        ),
        (
            StatusFilter::Stopped,
            count.map(|count| count.stopped),
            "icons/square-stop.svg",
            ColorToken::StatusStopped,
        ),
        (
            StatusFilter::Paused,
            count.map(|count| count.paused),
            "icons/pause.svg",
            ColorToken::StatusPaused,
        ),
        (
            StatusFilter::Restarting,
            count.map(|count| count.restarting),
            "icons/rotate-cw.svg",
            ColorToken::StatusStarting,
        ),
        (
            StatusFilter::Unhealthy,
            count.map(|count| count.unhealthy),
            "icons/triangle-alert.svg",
            ColorToken::StatusWarning,
        ),
        (
            StatusFilter::Updates,
            count
                .filter(|_| data.is_some_and(|data| data.updates_checked))
                .map(|count| count.updates),
            "icons/arrow-up-from-line.svg",
            ColorToken::TextMuted,
        ),
    ];
    let all_healthy = count.is_some_and(|count| {
        count.total > 0 && count.unhealthy == 0 && count.health_starting == 0 && count.healthy > 0
    });
    let issues = count.is_some_and(|count| count.unhealthy > 0 || count.health_starting > 0);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .pt_3()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(status::inline_badge(
            locale,
            screen.palette,
            status_state,
            status_label,
        ))
        .children(loading.then(|| quiet(screen).child(gui::GUI_OVERVIEW_MEASURING.text(locale))))
        .child(
            row(locale)
                .items_center()
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    row(locale)
                        .items_center()
                        .gap_4()
                        .flex_wrap()
                        .children(states.map(|(filter, value, glyph, token)| {
                            let token = if filter == StatusFilter::Updates
                                && value.is_some_and(|count| count > 0)
                            {
                                ColorToken::StatusWarning
                            } else {
                                token
                            };
                            status_stat(screen, filter, value, glyph, token, cx)
                        })),
                )
                .child(
                    row(locale)
                        .items_baseline()
                        .gap_2()
                        .flex_shrink_0()
                        .child(quiet(screen).child(gui::GUI_OVERVIEW_TOTAL.text(locale)))
                        .child(
                            screen
                                .typography(Role::Title)
                                .apply(div())
                                .child(number(count.map(|count| count.total))),
                        ),
                ),
        )
        .children(all_healthy.then(|| {
            row(locale)
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(10.))
                .bg(color(screen.palette, ColorToken::StatusRunningBg))
                .child(icon(
                    screen,
                    "icons/circle-check.svg",
                    ColorToken::StatusRunning,
                ))
                .child(
                    div()
                        .text_sm()
                        .font_weight(marsa_ui::gpui::FontWeight::SEMIBOLD)
                        .text_color(color(screen.palette, ColorToken::StatusRunning))
                        .child(gui::GUI_OVERVIEW_ALL_HEALTHY.text(locale)),
                )
        }))
        .children(issues.then(|| {
            Button::new("overview-unhealthy")
                .ghost()
                .w_full()
                .h_auto()
                .px_3()
                .py_2()
                .rounded(px(10.))
                .bg(color(screen.palette, ColorToken::StatusWarningBg))
                .child(
                    row(locale)
                        .items_center()
                        .gap_2()
                        .child(icon(
                            screen,
                            "icons/triangle-alert.svg",
                            ColorToken::StatusWarning,
                        ))
                        .child(
                            div()
                                .text_sm()
                                .text_color(color(screen.palette, ColorToken::StatusWarning))
                                .child(format!(
                                    "{} {}",
                                    number(count.map(|count| count.unhealthy)),
                                    gui::GUI_OVERVIEW_UNHEALTHY.text(locale)
                                )),
                        ),
                )
                .on_click(cx.listener(|desktop, _, window, cx| {
                    desktop.open_container_filter(StatusFilter::Unhealthy, window, cx);
                }))
        }))
}

fn status_stat(
    screen: &ScreenContext<'_>,
    filter: StatusFilter,
    value: Option<usize>,
    glyph: &'static str,
    token: ColorToken,
    cx: &Context<Desktop>,
) -> Button {
    let locale = screen.locale;
    let label = filter.label(locale);
    Button::new(("overview-container-filter", filter as usize))
        .ghost()
        .h_auto()
        .px_1()
        .py_1()
        .disabled(value.is_none())
        .accessibility_label(format!("{label} {}", number(value)))
        .tooltip(label)
        .child(
            row(locale)
                .items_center()
                .gap_2()
                .child(icon(screen, glyph, token))
                .child(
                    screen
                        .typography(Role::Body)
                        .apply(div())
                        .font_weight(marsa_ui::gpui::FontWeight::SEMIBOLD)
                        .child(number(value)),
                ),
        )
        .on_click(cx.listener(move |desktop, _, window, cx| {
            desktop.open_container_filter(filter, window, cx);
        }))
}

fn resources_card(
    screen: &ScreenContext<'_>,
    data: Option<&OverviewSample>,
    cx: &Context<Desktop>,
) -> Div {
    let locale = screen.locale;
    let resources = [
        (Route::Images, data.and_then(|data| data.images)),
        (Route::Volumes, data.and_then(|data| data.volumes)),
        (Route::Compose, data.and_then(|data| data.stacks)),
        (Route::Networks, data.and_then(|data| data.networks)),
    ];
    panel(screen)
        .min_w_0()
        .child(
            row(locale)
                .items_center()
                .gap_2()
                .child(icon(screen, "icons/layers.svg", ColorToken::TextMuted))
                .child(heading(screen, gui::GUI_OVERVIEW_RESOURCES.text(locale))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(resources.chunks(2).enumerate().map(|(row_index, pair)| {
                    row(locale)
                        .gap_2()
                        .w_full()
                        .children(pair.iter().enumerate().map(|(col, (route, value))| {
                            let route = *route;
                            let value = *value;
                            Button::new(("overview-resource", row_index * 2 + col))
                                .ghost()
                                .flex_1()
                                .h_auto()
                                .px_2()
                                .py_3()
                                .rounded(px(10.))
                                .bg(color(screen.palette, ColorToken::BgSubtle))
                                .tooltip(route.name(locale))
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_1()
                                        .child(icon(screen, route.icon(), ColorToken::TextMuted))
                                        .child(
                                            screen
                                                .typography(Role::Title)
                                                .apply(div())
                                                .child(number(value)),
                                        )
                                        .child(
                                            quiet(screen).text_center().child(route.name(locale)),
                                        ),
                                )
                                .on_click(cx.listener(move |desktop, _, _, cx| {
                                    desktop.navigate(route, cx);
                                }))
                        }))
                })),
        )
}

fn usage_card(
    screen: &ScreenContext<'_>,
    data: Option<&OverviewUtilization>,
    history: &UsageHistory,
    loading: bool,
) -> Div {
    let locale = screen.locale;
    let cpu = history.latest_engine(Metric::Cpu);
    let memory = history.latest_engine(Metric::Memory);
    let cpus = data.and_then(OverviewUtilization::online_cpus).unwrap_or(1);
    let memory_limit = data.and_then(OverviewUtilization::memory_limit);
    // Like the reference: percent on the title line, used/total on the subtitle.
    let cpu_primary = cpu.map(|value| format!("{:.1}%", value / f64::from(cpus)));
    let cpu_detail = cpu.map(|value| {
        format!(
            "{:.2} {} {cpus} {}",
            value / 100.,
            gui::GUI_OVERVIEW_OF.text(locale),
            gui::GUI_OVERVIEW_CPUS.text(locale)
        )
    });
    let memory_primary = memory.map(|bytes| match memory_limit {
        Some(limit) if limit > 0 => {
            format!("{:.1}%", (bytes / limit as f64 * 100.).clamp(0., 100.))
        }
        _ => "—".into(),
    });
    let memory_detail = memory.map(|bytes| match memory_limit {
        Some(limit) if limit > 0 => format!(
            "{} {} {}",
            disk_bytes(locale, bytes as u64),
            gui::GUI_OVERVIEW_OF.text(locale),
            disk_bytes(locale, limit)
        ),
        _ => disk_bytes(locale, bytes as u64),
    });
    let cpu_ceiling = Some(f64::from(cpus) * 100.);
    let memory_ceiling = memory_limit
        .filter(|limit| *limit > 0)
        .map(|limit| limit as f64);
    let cpu_fraction = cpu.map(|value| (value / f64::from(cpus) / 100.).clamp(0., 1.));
    let memory_fraction = memory.and_then(|bytes| {
        memory_limit
            .filter(|limit| *limit > 0)
            .map(|limit| (bytes / limit as f64).clamp(0., 1.))
    });
    panel(screen)
        .min_w_0()
        .overflow_hidden()
        .child(
            row(locale)
                .items_center()
                .gap_2()
                .child(icon(screen, "icons/cpu.svg", ColorToken::TextMuted))
                .child(heading(
                    screen,
                    gui::GUI_OVERVIEW_CPU_MEMORY_HISTORY.text(locale),
                )),
        )
        .children(loading.then(|| quiet(screen).child(gui::GUI_OVERVIEW_MEASURING.text(locale))))
        .child(metric_history(
            screen,
            gui::GUI_OVERVIEW_CPU.text(locale),
            Metric::Cpu,
            MetricLabels {
                primary: cpu_primary,
                detail: cpu_detail,
                fraction: cpu_fraction,
                ceiling: cpu_ceiling,
            },
            history,
        ))
        .child(metric_history(
            screen,
            gui::GUI_SYSTEM_MACHINE_MEMORY.text(locale),
            Metric::Memory,
            MetricLabels {
                primary: memory_primary,
                detail: memory_detail,
                fraction: memory_fraction,
                ceiling: memory_ceiling,
            },
            history,
        ))
}

struct MetricLabels {
    primary: Option<String>,
    detail: Option<String>,
    fraction: Option<f64>,
    ceiling: Option<f64>,
}

fn metric_history(
    screen: &ScreenContext<'_>,
    label: &'static str,
    metric: Metric,
    labels: MetricLabels,
    history: &UsageHistory,
) -> Div {
    let locale = screen.locale;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .min_w_0()
        .child(
            row(locale)
                .items_start()
                .justify_between()
                .gap_3()
                .child(quiet(screen).pt_1().child(label))
                .child(
                    // Keep percent + capacity stacked on the trailing edge.
                    align_column_to_end(div().min_w_0().flex().flex_col().gap_0(), locale)
                        .child(
                            screen
                                .typography(Role::Title)
                                .apply(div())
                                .whitespace_nowrap()
                                .child(
                                    labels
                                        .primary
                                        .map_or_else(|| technical("—"), |value| technical(&value)),
                                ),
                        )
                        .children(labels.detail.map(|value| {
                            quiet(screen).whitespace_nowrap().child(technical(&value))
                        })),
                ),
        )
        .child(capacity_bar(screen, labels.fraction, metric.color()))
        .child(history.engine_line(screen, metric, labels.ceiling))
}

fn capacity_bar(screen: &ScreenContext<'_>, fraction: Option<f64>, token: ColorToken) -> Div {
    let filled = fraction.unwrap_or(0.).clamp(0., 1.) as f32;
    div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(color(screen.palette, ColorToken::BgSubtle))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(filled))
                .min_w(if filled > 0. { px(2.) } else { px(0.) })
                .rounded_full()
                .bg(color(screen.palette, token)),
        )
}

fn top_containers(
    screen: &ScreenContext<'_>,
    data: Option<&OverviewUtilization>,
    history: &UsageHistory,
    loading: bool,
    wide: bool,
    cx: &Context<Desktop>,
) -> Div {
    let locale = screen.locale;
    let mut card = panel(screen).child(heading(
        screen,
        gui::GUI_OVERVIEW_TOP_CONTAINERS.text(locale),
    ));
    let Some(data) = data else {
        return card.child(quiet(screen).child(if loading {
            gui::GUI_OVERVIEW_MEASURING.text(locale)
        } else {
            gui::GUI_DESKTOP_UNAVAILABLE.text(locale)
        }));
    };
    if data.running == 0 {
        return card.child(quiet(screen).child(gui::GUI_OVERVIEW_NO_RUNNING.text(locale)));
    }
    let mut by_cpu: Vec<_> = data
        .usage
        .iter()
        .filter(|entry| entry.cpu_percent.is_some())
        .collect();
    by_cpu.sort_by(|a, b| {
        b.cpu_percent
            .unwrap_or_default()
            .total_cmp(&a.cpu_percent.unwrap_or_default())
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut by_memory: Vec<_> = data
        .usage
        .iter()
        .filter(|entry| entry.memory_bytes.is_some())
        .collect();
    by_memory.sort_by(|a, b| {
        b.memory_bytes
            .cmp(&a.memory_bytes)
            .then_with(|| a.id.cmp(&b.id))
    });
    let columns = if wide {
        row(locale).items_start()
    } else {
        div().flex().flex_col()
    };
    card = card.child(
        columns
            .gap_4()
            .child(ranking(
                screen,
                gui::GUI_OVERVIEW_CPU.text(locale),
                by_cpu,
                history,
                Metric::Cpu,
                cx,
            ))
            .child(ranking(
                screen,
                gui::GUI_SYSTEM_MACHINE_MEMORY.text(locale),
                by_memory,
                history,
                Metric::Memory,
                cx,
            )),
    );
    card
}

fn ranking(
    screen: &ScreenContext<'_>,
    label: &'static str,
    entries: Vec<&ContainerUtilization>,
    history: &UsageHistory,
    metric: Metric,
    cx: &Context<Desktop>,
) -> Div {
    let locale = screen.locale;
    let measured = entries.len();
    align(div().flex_1().min_w_0().flex().flex_col().gap_2(), locale)
        .child(div().text_sm().child(label))
        .children(
            (measured == 0).then(|| quiet(screen).child(gui::GUI_DESKTOP_UNAVAILABLE.text(locale))),
        )
        .children(
            entries
                .into_iter()
                .take(4)
                .enumerate()
                .filter_map(|(index, entry)| {
                    let usage = match metric {
                        Metric::Cpu => format!("{:.1}%", entry.cpu_percent?),
                        Metric::Memory => disk_bytes(locale, entry.memory_bytes?),
                    };
                    let id = entry.id.clone();
                    let name = entry.name.clone();
                    let chart_key = match metric {
                        Metric::Cpu => 0_usize,
                        Metric::Memory => 1,
                    };
                    Some(
                        row(locale)
                            .gap_3()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .pt_2()
                            .border_color(color(screen.palette, ColorToken::BorderSubtle))
                            .child(
                                Button::new(("overview-top-container", chart_key * 8 + index))
                                    .ghost()
                                    .flex_1()
                                    .h_auto()
                                    .px_0()
                                    .py_0()
                                    .min_w_0()
                                    .tooltip(name.as_str())
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .text_color(color(
                                                screen.palette,
                                                ColorToken::AccentDefault,
                                            ))
                                            .child(technical(&name)),
                                    )
                                    .on_click(cx.listener(move |desktop, _, _, cx| {
                                        desktop.open_overview_container(id.clone(), cx);
                                    })),
                            )
                            .child(history.bars(screen, &entry.id, metric))
                            .child(mono(screen).flex_shrink_0().child(technical(&usage))),
                    )
                }),
        )
}

fn disk_donut(segments: Vec<(Hsla, f64)>) -> impl IntoElement {
    let size = px(128.);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _: &mut App| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let outer = width.min(height) * 0.48;
            let inner = outer * 0.62;
            if outer <= 0. || segments.is_empty() {
                return;
            }
            let cx = bounds.origin.x + bounds.size.width / 2.;
            let cy = bounds.origin.y + bounds.size.height / 2.;
            let gap = 0.05_f32;
            let mut angle = -std::f32::consts::FRAC_PI_2;
            for (tint, fraction) in &segments {
                let full = (*fraction as f32 * std::f32::consts::TAU).max(0.014);
                let sweep = (full - gap).max(0.01);
                let start = angle + gap * 0.5;
                let end = start + sweep;
                let outer_start = point(cx + px(outer * start.cos()), cy + px(outer * start.sin()));
                let outer_end = point(cx + px(outer * end.cos()), cy + px(outer * end.sin()));
                let inner_end = point(cx + px(inner * end.cos()), cy + px(inner * end.sin()));
                let inner_start = point(cx + px(inner * start.cos()), cy + px(inner * start.sin()));
                let mut path = PathBuilder::fill();
                path.move_to(outer_start);
                path.arc_to(
                    point(px(outer), px(outer)),
                    px(0.),
                    sweep > std::f32::consts::PI,
                    true,
                    outer_end,
                );
                path.line_to(inner_end);
                path.arc_to(
                    point(px(inner), px(inner)),
                    px(0.),
                    sweep > std::f32::consts::PI,
                    false,
                    inner_start,
                );
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, *tint);
                }
                angle += full;
            }
        },
    )
    .size(size)
    .flex_shrink_0()
}

fn heading(screen: &ScreenContext<'_>, label: &'static str) -> Div {
    screen
        .typography(Role::Body)
        .apply(div())
        .font_weight(marsa_ui::gpui::FontWeight::SEMIBOLD)
        .child(label)
}

fn icon(
    screen: &ScreenContext<'_>,
    path: &'static str,
    foreground: ColorToken,
) -> marsa_ui::gpui::Svg {
    svg()
        .path(path)
        .size(px(18.))
        .text_color(color(screen.palette, foreground))
}

fn quiet(screen: &ScreenContext<'_>) -> Div {
    align(
        div()
            .text_xs()
            .text_color(color(screen.palette, ColorToken::TextMuted)),
        screen.locale,
    )
}

fn mono(screen: &ScreenContext<'_>) -> Div {
    quiet(screen).font_family("IBM Plex Mono")
}

fn number(count: Option<usize>) -> marsa_ui::gpui::SharedString {
    count.map_or_else(|| technical("—"), |count| technical(&count.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_results_keep_their_engine_epoch_and_image_inventory() {
        use marsa_client::docker::DockerInventory;

        let state = AppState::index_resources(DockerInventory::default());
        let original = observed_images(&state).unwrap();
        let expected = Target::Marsa(state.docker_epoch());
        assert!(current_image_observations(&expected, &state, &original));
        assert!(!current_image_observations(
            &Target::Marsa(state.docker_epoch() + 1),
            &state,
            &original,
        ));
        assert!(!current_image_observations(
            &expected,
            &AppState::default(),
            &original,
        ));

        let changed = AppState::index_resources(DockerInventory {
            images: vec![ImageSummary {
                id: "sha256:".to_owned() + &"a".repeat(64),
                repo_tags: vec!["alpine:latest".to_owned()],
                repo_digests: Vec::new(),
                created: 0,
                size: 0,
            }],
            ..DockerInventory::default()
        });
        assert!(!current_image_observations(&expected, &changed, &original));
    }

    #[test]
    fn inactive_overview_can_start_a_stopped_engine_or_stop_a_paused_engine() {
        for (state, expected) in [
            (EngineState::Stopped, EngineAction::Start),
            (EngineState::Paused, EngineAction::Stop),
        ] {
            assert!(
                overview_power_action(Some(state)).is_some_and(|(action, _, _)| action == expected)
            );
        }
        assert!(overview_power_action(Some(EngineState::Starting)).is_none());
    }
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use marsa_client::docker::{DockerInventory, VolumeSummary};

    #[test]
    fn cached_and_invalidated_counts_preserve_live_measurements() {
        let mut overview = Overview {
            target: Some(Target::Marsa(0)),
            ..Overview::default()
        };
        overview.sync_inventory(&AppState::default());
        assert!(overview.sample.is_none());

        overview.sync_inventory(&AppState::index_resources(DockerInventory::default()));
        let cached = overview.sample.as_mut().unwrap();
        assert_eq!(cached.containers.as_ref().unwrap().total, 0);
        assert_eq!(cached.volumes, Some(0));
        cached.utilization.as_mut().unwrap().running = 7;
        cached.updates_checked = true;

        let mut inventory = DockerInventory::default();
        inventory.volumes.push(VolumeSummary {
            name: "observed-volume".into(),
            driver: "local".into(),
            scope: "local".into(),
            created_at: None,
        });
        let changed = AppState::index_resources(inventory);
        overview.sync_inventory(&changed);
        let current = overview.sample.as_ref().unwrap();
        assert_eq!(current.volumes, Some(1));
        assert_eq!(current.utilization.as_ref().unwrap().running, 7);
        assert!(current.updates_checked);

        overview.target = Some(Target::Marsa(1));
        overview.sample = None;
        overview.sync_inventory(&changed);
        assert!(overview.sample.is_none());
    }
}
