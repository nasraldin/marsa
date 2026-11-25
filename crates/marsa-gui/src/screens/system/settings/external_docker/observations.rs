//! Read current events and inventory for the active environment.

use marsa_client::{
    api::{DiskCategory, EngineState, PrunePreviewRequest, PrunePreviewResponse},
    docker::{DockerActivity, EnvironmentReport, ExternalEndpoint},
};
use marsa_ui::{
    ColorToken, TextKey,
    gpui::{
        ClipboardItem, Context, Div, ParentElement as _, SharedString, Styled as _, Task, div, px,
    },
    gpui_component::{Disableable as _, IconName, button::Button},
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_text};
use crate::screens::{
    common::{ScreenContext, color, panel, row, technical},
    system::activity::utc_time,
};

#[derive(Default)]
pub(in crate::screens::system) struct Observations {
    target: Option<Target>,
    page: Option<Page>,
    generation: u64,
    reading: Load<ResultPage>,
    task: Option<Task<()>>,
}

#[derive(Default)]
enum Load<T> {
    #[default]
    Empty,
    Loading,
    Ready(T),
    Failed,
}

#[derive(Clone, PartialEq, Eq)]
enum Target {
    Marsa,
    External(Box<ExternalEndpoint>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Activity(u32),
    Report,
    Disk,
}

enum ResultPage {
    Activity(Vec<DockerActivity>),
    Report(EnvironmentReport),
    Disk(PrunePreviewResponse),
}

impl Observations {
    fn select(&mut self, target: Option<Target>, page: Option<Page>) {
        if self.target != target || self.page != page {
            *self = Self {
                target,
                page,
                generation: self.generation.wrapping_add(1),
                ..Self::default()
            };
        }
    }

    pub(in crate::screens::system) fn clear(&mut self) {
        self.select(None, None);
    }

    fn current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

impl SystemView {
    pub(crate) fn environment_observations_page(
        &mut self,
        report: bool,
        local: bool,
        target: Option<ExternalEndpoint>,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        if local {
            self.environment_observations.clear();
            let ready = screen.state.engine().is_some_and(|engine| {
                matches!(engine.state(), EngineState::Ready | EngineState::Idle)
            });
            return if ready {
                let containers = screen.state.containers();
                let entries: Vec<_> = containers.values().cloned().collect();
                let snapshot = EnvironmentReport::from_inventory(
                    &entries,
                    screen.state.images().len(),
                    Some(screen.state.volumes().len()),
                    Some(screen.state.networks().len()),
                );
                report_body(screen, div().flex().flex_col().gap_4(), &snapshot)
            } else {
                settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_DESKTOP_RESOURCES_ARE_AVAILABLE_WHEN_THE_ENGINE_IS_RUNNING
                        .text(screen.locale),
                    ColorToken::TextMuted,
                )
            };
        }
        let page = if report {
            Page::Report
        } else {
            let preferences = self.monitoring();
            if !preferences.collect_activity {
                return self.activity_disabled(screen);
            }
            Page::Activity(preferences.activity_retention.hours())
        };
        self.environment_observations.select(
            target
                .clone()
                .map(|endpoint| Target::External(Box::new(endpoint))),
            Some(page),
        );
        if target.is_none() {
            return self.offline_observations(screen, cx);
        }
        if matches!(self.environment_observations.reading, Load::Empty) {
            self.load_environment_observations(cx);
        }
        if report {
            self.environment_report(screen, cx)
        } else {
            self.environment_activity(screen, cx)
        }
    }

    pub(crate) fn environment_disk_page(
        &mut self,
        target: Option<ExternalEndpoint>,
        name: &str,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        self.environment_observations.select(
            target
                .clone()
                .map(|endpoint| Target::External(Box::new(endpoint))),
            Some(Page::Disk),
        );
        let Some(target) = target else {
            return div();
        };
        if matches!(self.environment_observations.reading, Load::Empty) {
            self.load_environment_observations(cx);
        }
        let page = &self.environment_observations.reading;
        let body = self.observation_header(
            screen,
            gui::GUI_ENVIRONMENT_DISK_HELP,
            matches!(page, Load::Loading),
            cx,
        );
        if let Load::Ready(ResultPage::Disk(preview)) = page {
            return body.child(crate::screens::system::disk::external_cleanup(
                screen,
                (name.to_owned(), target.address()),
                preview,
                self.monitoring().show_volume_candidates,
            ));
        }
        body.children(status(
            screen,
            page,
            false,
            gui::GUI_SYSTEM_DISK_INVENTORY_UNAVAILABLE,
        ))
    }

    pub(in crate::screens::system) fn managed_activity_panel(
        &mut self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let preferences = self.monitoring();
        if !preferences.collect_activity {
            return self.activity_disabled(screen);
        }
        let ready = screen
            .state
            .engine()
            .is_some_and(|engine| matches!(engine.state(), EngineState::Ready | EngineState::Idle));
        self.environment_observations.select(
            ready.then_some(Target::Marsa),
            Some(Page::Activity(preferences.activity_retention.hours())),
        );
        if !ready {
            return settings_text(
                screen,
                Role::Small,
                gui::GUI_DESKTOP_RESOURCES_ARE_AVAILABLE_WHEN_THE_ENGINE_IS_RUNNING
                    .text(screen.locale),
                ColorToken::TextMuted,
            );
        }
        if matches!(self.environment_observations.reading, Load::Empty) {
            self.load_environment_observations(cx);
        }
        self.environment_activity(screen, cx)
    }

    fn activity_disabled(&mut self, screen: &ScreenContext<'_>) -> Div {
        self.environment_observations.clear();
        settings_text(
            screen,
            Role::Small,
            gui::GUI_ENVIRONMENT_MONITORING_ACTIVITY_DISABLED.text(screen.locale),
            ColorToken::TextMuted,
        )
    }

    fn offline_observations(&self, screen: &ScreenContext<'_>, cx: &Context<Self>) -> Div {
        row(screen.locale)
            .gap_3()
            .items_center()
            .child(settings_text(
                screen,
                Role::Small,
                gui::GUI_ENVIRONMENT_OFFLINE.text(screen.locale),
                ColorToken::TextMuted,
            ))
            .child(
                Button::new("retry-environment-connection")
                    .label(gui::GUI_DESKTOP_REFRESH.text(screen.locale))
                    .disabled(self.external_inventory_pending())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_external_inventory(cx))),
            )
    }

    fn load_environment_observations(&mut self, cx: &mut Context<Self>) {
        const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
        const DISK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
        const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        let Some(services) = &self.services else {
            return;
        };
        let observations = &mut self.environment_observations;
        let (Some(target), Some(page)) = (observations.target.clone(), observations.page) else {
            return;
        };
        if observations.task.is_some() {
            return;
        }
        let generation = observations.generation;
        observations.reading = Load::Loading;
        let services = std::sync::Arc::clone(services);
        let timer = cx.background_executor().clone();
        observations.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = match (page, &target) {
                    (Page::Activity(hours), target) => {
                        let endpoint = match target {
                            Target::Marsa => None,
                            Target::External(endpoint) => Some(endpoint.as_ref().clone()),
                        };
                        services
                            .engine_activity(endpoint, hours)
                            .result()
                            .await
                            .and_then(Result::ok)
                            .map(ResultPage::Activity)
                    }
                    (Page::Report, Target::External(endpoint)) => {
                        let docker = endpoint.management_client();
                        services
                            .pending(async move {
                                let containers = docker.containers().await?;
                                let images = docker.images().await?;
                                let volumes = docker.volumes().await.ok().map(|items| items.len());
                                let networks =
                                    docker.networks().await.ok().map(|items| items.len());
                                Ok(ResultPage::Report(EnvironmentReport::from_inventory(
                                    &containers,
                                    images.len(),
                                    volumes,
                                    networks,
                                )))
                            })
                            .result()
                            .await
                            .and_then(Result::ok)
                    }
                    (Page::Disk, Target::External(endpoint)) => {
                        let docker = endpoint.management_client();
                        services
                            .pending(async move {
                                let preview = docker
                                    .cleanup_preview(PrunePreviewRequest {
                                        categories: [
                                            DiskCategory::Containers,
                                            DiskCategory::Images,
                                            DiskCategory::Volumes,
                                            DiskCategory::BuildCache,
                                        ]
                                        .into_iter()
                                        .map(i32::from)
                                        .collect(),
                                        volume_names: Vec::new(),
                                    })
                                    .await?;
                                Ok(ResultPage::Disk(preview))
                            })
                            .result()
                            .await
                            .and_then(Result::ok)
                    }
                    (_, Target::Marsa) => return,
                };
                let retry = result.is_none();
                if this
                    .update(cx, |this, cx| {
                        let observations = &mut this.environment_observations;
                        if !observations.current(generation) {
                            return false;
                        }
                        observations.reading = result.map_or(Load::Failed, Load::Ready);
                        cx.notify();
                        true
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
                let interval = match (retry, page) {
                    (true, _) => RETRY_INTERVAL,
                    (false, Page::Disk) => DISK_INTERVAL,
                    (false, _) => REFRESH_INTERVAL,
                };
                timer.timer(interval).await;
            }
        }));
        cx.notify();
    }

    pub(super) fn environment_activity(
        &self,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let page = &self.environment_observations.reading;
        let period = match self.environment_observations.page {
            Some(Page::Activity(1)) => gui::GUI_ENVIRONMENT_MONITORING_PERIOD_ONE_HOUR,
            Some(Page::Activity(6)) => gui::GUI_ENVIRONMENT_MONITORING_PERIOD_SIX_HOURS,
            _ => gui::GUI_ENVIRONMENT_ACTIVITY_PERIOD,
        };
        let mut body = self.observation_header(screen, period, matches!(page, Load::Loading), cx);
        if let Load::Ready(ResultPage::Activity(events)) = page
            && !events.is_empty()
        {
            body = body.child(panel(screen).gap_1().children(events.iter().map(|event| {
                let time = utc_time(
                    screen.locale,
                    screen.display.time_format,
                    event.timestamp.saturating_mul(1000),
                );
                detail(
                    screen,
                    &event.subject,
                    &format!("{} / {}", event.kind, event.action),
                    technical(&time),
                    ColorToken::TextMuted,
                )
            })));
        }
        body.children(status(
            screen,
            page,
            matches!(page, Load::Ready(ResultPage::Activity(events)) if events.is_empty()),
            gui::GUI_ENVIRONMENT_NO_EVENTS,
        ))
    }

    pub(super) fn environment_report(&self, screen: &ScreenContext<'_>, cx: &Context<Self>) -> Div {
        let page = &self.environment_observations.reading;
        let mut body = self.observation_header(
            screen,
            gui::GUI_ENVIRONMENT_REPORT_HELP,
            matches!(page, Load::Loading),
            cx,
        );
        if let Load::Ready(ResultPage::Report(snapshot)) = page {
            body = report_body(screen, body, snapshot);
        }
        body.children(status(screen, page, false, gui::GUI_ENVIRONMENT_NO_EVENTS))
    }

    fn observation_header(
        &self,
        screen: &ScreenContext<'_>,
        help: TextKey,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        div().flex().flex_col().gap_4().child(
            row(screen.locale)
                .gap_3()
                .flex_wrap()
                .items_center()
                .child(
                    settings_text(
                        screen,
                        Role::Small,
                        help.text(screen.locale),
                        ColorToken::TextMuted,
                    )
                    .flex_1()
                    .min_w(px(200.)),
                )
                .child(
                    Button::new("refresh-environment-observations")
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label(gui::GUI_DESKTOP_REFRESH.text(screen.locale))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let observations = &mut this.environment_observations;
                            observations.generation = observations.generation.wrapping_add(1);
                            observations.task = None;
                            if matches!(observations.target, Some(Target::External(_))) {
                                this.refresh_external_inventory(cx);
                            }
                            this.load_environment_observations(cx);
                        })),
                ),
        )
    }
}

fn report_body(screen: &ScreenContext<'_>, body: Div, snapshot: &EnvironmentReport) -> Div {
    let locale = screen.locale;
    let counts = [
        (gui::GUI_DESKTOP_CONTAINERS, Some(snapshot.containers.len())),
        (gui::GUI_DESKTOP_IMAGES, Some(snapshot.image_count)),
        (gui::GUI_DESKTOP_VOLUMES, snapshot.volume_count),
        (gui::GUI_DESKTOP_NETWORKS, snapshot.network_count),
    ]
    .map(|(key, count)| {
        let value = count.map_or_else(
            || gui::GUI_DESKTOP_UNAVAILABLE.text(locale).to_owned(),
            |count| count.to_string(),
        );
        (key.text(locale), value)
    });
    let mut clipboard = counts
        .iter()
        .map(|(label, value)| format!("{label}  {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut containers = snapshot.containers.iter().collect::<Vec<_>>();
    containers.sort_by(|first, second| first.name.cmp(&second.name));
    for container in &containers {
        use std::fmt::Write as _;
        let _ = write!(
            clipboard,
            "\n{}\t{}\t{}",
            container.name, container.image, container.state
        );
    }

    body.child(
        panel(screen)
            .child(
                row(locale)
                    .gap_3()
                    .flex_wrap()
                    .justify_between()
                    .child(settings_text(
                        screen,
                        Role::Lead,
                        gui::GUI_ENVIRONMENT_REPORT_SUMMARY.text(locale),
                        ColorToken::TextPrimary,
                    ))
                    .child(
                        Button::new("copy-environment-report")
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_REPORT_COPY.text(locale))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(clipboard.clone()))
                            }),
                    ),
            )
            .child(
                row(locale)
                    .gap_3()
                    .flex_wrap()
                    .children(counts.map(|(label, value)| {
                        div()
                            .min_w(px(120.))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .border_t_1()
                            .border_color(color(screen.palette, ColorToken::BorderSubtle))
                            .child(settings_text(
                                screen,
                                Role::Small,
                                label,
                                ColorToken::TextMuted,
                            ))
                            .child(settings_text(
                                screen,
                                Role::Title,
                                value,
                                ColorToken::TextPrimary,
                            ))
                    })),
            ),
    )
    .child(
        panel(screen)
            .gap_2()
            .child(settings_text(
                screen,
                Role::Lead,
                gui::GUI_DESKTOP_CONTAINERS.text(locale),
                ColorToken::TextPrimary,
            ))
            .children(containers.is_empty().then(|| {
                settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_ENVIRONMENT_REPORT_NO_CONTAINERS.text(locale),
                    ColorToken::TextMuted,
                )
            }))
            .children(containers.into_iter().map(|container| {
                let (label, tone) = match container.state.as_str() {
                    "running" => (
                        gui::GUI_DESKTOP_RUNNING.text(locale).into(),
                        ColorToken::StatusRunning,
                    ),
                    "exited" | "dead" => (
                        gui::GUI_DESKTOP_STOPPED.text(locale).into(),
                        ColorToken::TextMuted,
                    ),
                    "paused" => (
                        gui::GUI_DESKTOP_PAUSED.text(locale).into(),
                        ColorToken::StatusWarning,
                    ),
                    _ => (technical(&container.state), ColorToken::TextMuted),
                };
                detail(screen, &container.name, &container.image, label, tone)
            })),
    )
}

fn detail(
    screen: &ScreenContext<'_>,
    title: &str,
    metadata: &str,
    annotation: SharedString,
    tone: ColorToken,
) -> Div {
    row(screen.locale)
        .flex_wrap()
        .justify_between()
        .gap_2()
        .py_2()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w(px(185.))
                .child(
                    settings_text(
                        screen,
                        Role::Label,
                        technical(title),
                        ColorToken::TextPrimary,
                    )
                    .truncate(),
                )
                .child(
                    settings_text(
                        screen,
                        Role::Small,
                        technical(metadata),
                        ColorToken::TextMuted,
                    )
                    .truncate(),
                ),
        )
        .child(settings_text(screen, Role::Small, annotation, tone))
}

fn status<T>(
    screen: &ScreenContext<'_>,
    page: &Load<T>,
    empty: bool,
    empty_key: TextKey,
) -> Option<Div> {
    let (label, token) = match page {
        Load::Empty | Load::Loading => (gui::GUI_DESKTOP_WORKING, ColorToken::TextMuted),
        Load::Ready(_) if empty => (empty_key, ColorToken::TextMuted),
        Load::Ready(_) => return None,
        Load::Failed => (gui::GUI_DESKTOP_UNAVAILABLE, ColorToken::StatusWarning),
    };
    Some(settings_text(
        screen,
        Role::Small,
        label.text(screen.locale),
        token,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_and_returning_rejects_an_earlier_read_of_the_same_page() {
        let mut observations = Observations::default();
        observations.select(None, Some(Page::Activity(24)));
        let first = observations.generation;
        observations.select(None, Some(Page::Disk));
        observations.select(None, Some(Page::Activity(24)));
        assert!(!observations.current(first));
        assert!(observations.current(observations.generation));
    }
}
