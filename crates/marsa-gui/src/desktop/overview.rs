//! Observe only the selected engine while the Overview is visible.

use std::time::Duration;

use marsa_client::{
    api::EngineState,
    docker::{
        DiskBreakdown, ExternalEndpoint, OverviewClient, OverviewSample, OverviewUtilization,
    },
};
use marsa_ui::{
    ColorToken, TextKey,
    gpui::{Context, Div, ParentElement as _, Styled as _, Task, Window, div, px, relative, svg},
    gpui_component::{
        IconName,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    preferences::EnvironmentIcon,
    tokens::typography::Role,
};

use crate::screens::{
    common::{ScreenContext, align, color, disk_bytes, panel, row, technical},
    external_inventory::Environment,
    system::environment_icon_path,
};

use super::{Desktop, Route, status};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(8);
const DISK_INTERVAL: Duration = Duration::from_secs(60);
const DISK_RETRY: Duration = Duration::from_secs(15);

#[derive(Clone, PartialEq, Eq)]
enum Target {
    Marsa,
    External(Box<ExternalEndpoint>),
    Checking(Environment),
}

#[derive(Default)]
pub(super) struct Overview {
    target: Option<Target>,
    task: Option<Task<()>>,
    disk_task: Option<Task<()>>,
    sample: Option<OverviewSample>,
    disk: Option<DiskBreakdown>,
    disk_pending: bool,
}

impl Desktop {
    pub(super) fn prepare_overview(&mut self, cx: &mut Context<Self>) {
        let selection = self.environment.read(cx);
        let target = if self.route != Route::Overview || self.onboarding.is_some() {
            None
        } else if selection.includes_marsa() {
            matches!(self.engine(), Some(EngineState::Ready | EngineState::Idle))
                .then_some(Target::Marsa)
        } else {
            Some(self.active_workspace(cx).map_or_else(
                || Target::Checking(selection.clone()),
                |workspace| Target::External(Box::new(workspace.endpoint.clone())),
            ))
        };
        if self.overview.target == target {
            return;
        }
        self.overview = Overview {
            disk_pending: matches!(target, Some(Target::Marsa | Target::External(_))),
            target: target.clone(),
            ..Overview::default()
        };
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
                loop {
                    let disk = match &storage_target {
                        Target::Marsa => {
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
                        Target::Checking(_) => break,
                    };
                    let interval = if disk.is_some() {
                        DISK_INTERVAL
                    } else {
                        DISK_RETRY
                    };
                    if this
                        .update(cx, |this, cx| {
                            if this.overview.target.as_ref() != Some(&storage_target) {
                                return false;
                            }
                            this.overview.disk = disk;
                            this.overview.disk_pending = false;
                            cx.notify();
                            true
                        })
                        .ok()
                        != Some(true)
                    {
                        break;
                    }
                    storage_executor.timer(interval).await;
                }
            }));
        }
        self.overview.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let sample = match &target {
                    Target::Marsa => {
                        let mut request = services.request(|client| async move {
                            Ok(OverviewClient::Marsa(client).sample().await)
                        });
                        request.result().await.and_then(Result::ok)
                    }
                    Target::External(endpoint) => {
                        let client = OverviewClient::external(endpoint);
                        let mut request =
                            services.pending(async move { Ok(client.sample().await) });
                        request.result().await.and_then(Result::ok)
                    }
                    Target::Checking(_) => None,
                };
                if this
                    .update(cx, |this, cx| {
                        if this.overview.target.as_ref() != Some(&target) {
                            return false;
                        }
                        let sample = sample.unwrap_or_default();
                        let connected = observed(&sample);
                        if !connected && !matches!(target, Target::Marsa) {
                            this.system
                                .update(cx, |system, cx| system.refresh_external_inventory(cx));
                        }
                        this.overview.sample = Some(sample);
                        cx.notify();
                        true
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
                executor.timer(SAMPLE_INTERVAL).await;
            }
        }));
    }

    pub(super) fn overview_screen(
        &self,
        screen: &ScreenContext<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let selection = self.environment.read(cx);
        let external = self.active_workspace(cx);
        let data = self.overview.sample.as_ref();
        let engine = if !selection.includes_marsa()
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
        let (name, glyph) = if selection.includes_marsa() || workspace.is_some() {
            let (name, icon) = self
                .system
                .read(cx)
                .environment_presentation(workspace.map(AsRef::as_ref));
            (technical(&name), icon)
        } else {
            (selection.display(), EnvironmentIcon::Server)
        };
        let address = workspace.map(|workspace| workspace.endpoint.address());
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
        let refresh = Button::new("overview-refresh")
            .ghost()
            .size(px(36.))
            .p_0()
            .child(IconName::RefreshCw)
            .accessibility_label(gui::GUI_DESKTOP_REFRESH.text(locale))
            .tooltip(gui::GUI_DESKTOP_REFRESH.text(locale))
            .on_click(cx.listener(|desktop, _, _, cx| {
                desktop.overview = Overview::default();
                if !desktop.environment.read(cx).includes_marsa() {
                    desktop
                        .system
                        .update(cx, |system, cx| system.refresh_external_inventory(cx));
                }
                cx.notify();
            }));
        let identity = panel(screen)
            .bg(color(screen.palette, ColorToken::GlassRail))
            .child(
                row(locale)
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .size(px(42.))
                            .flex_shrink_0()
                            .rounded(px(10.))
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
                        align(div().flex_1().min_w_0().flex().flex_col().gap_1(), locale)
                            .child(
                                row(locale)
                                    .gap_3()
                                    .flex_wrap()
                                    .child(
                                        screen
                                            .typography(Role::Title)
                                            .apply(div().min_w_0())
                                            .truncate()
                                            .child(name),
                                    )
                                    .child(status::inline_badge(
                                        locale,
                                        screen.palette,
                                        engine,
                                        self.context_status_text(engine, cx),
                                    )),
                            )
                            .child(
                                row(locale)
                                    .gap_2()
                                    .flex_wrap()
                                    .child(
                                        quiet(screen)
                                            .child(gui::GUI_DESKTOP_OVERVIEW_RUNTIME.text(locale)),
                                    )
                                    .children(
                                        version_text
                                            .map(|value| quiet(screen).child(technical(value))),
                                    )
                                    .children(
                                        platform
                                            .as_ref()
                                            .map(|value| quiet(screen).child(technical(value))),
                                    ),
                            ),
                    )
                    .child(
                        row(locale).gap_1().flex_shrink_0().child(refresh).child(
                            Button::new("overview-manage-environments")
                                .ghost()
                                .size(px(36.))
                                .p_0()
                                .child(IconName::Settings2)
                                .accessibility_label(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
                                .tooltip(gui::GUI_ENVIRONMENT_MANAGE.text(locale))
                                .on_click(
                                    cx.listener(|desktop, _, _, cx| desktop.open_contexts(cx)),
                                ),
                        ),
                    ),
            )
            .children(
                address
                    .as_ref()
                    .map(|address| quiet(screen).truncate().child(technical(address))),
            );
        let resources = row(locale).flex_wrap().gap_3().children(
            [
                (Route::Images, data.and_then(|data| data.images)),
                (Route::Volumes, data.and_then(|data| data.volumes)),
                (Route::Networks, data.and_then(|data| data.networks)),
                (Route::Compose, data.and_then(|data| data.stacks)),
            ]
            .map(|(route, value)| {
                Button::new(("overview-resource", route as usize))
                    .outline()
                    .min_w(px(130.))
                    .flex_1()
                    .h_auto()
                    .border_color(color(screen.palette, ColorToken::BorderSubtle))
                    .bg(color(screen.palette, ColorToken::BgElevated))
                    .child(
                        row(locale)
                            .w_full()
                            .my_2()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(32.))
                                    .flex_shrink_0()
                                    .rounded(px(8.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(color(screen.palette, ColorToken::BgSubtle))
                                    .child(icon(screen, route.icon(), ColorToken::TextMuted)),
                            )
                            .child(
                                align(div().min_w_0().flex().flex_col().gap_1(), locale)
                                    .child(quiet(screen).truncate().child(route.name(locale)))
                                    .child(
                                        screen
                                            .typography(Role::Title)
                                            .apply(div())
                                            .child(number(value)),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(move |desktop, _, _, cx| desktop.navigate(route, cx)))
            }),
        );
        let wide = window.viewport_size().width >= px(if self.collapsed { 800. } else { 1000. });
        let columns = if wide {
            row(locale).items_start()
        } else {
            div().flex().flex_col()
        };
        let loading = data.is_none()
            && matches!(
                self.overview.target,
                Some(Target::Marsa | Target::External(_))
            );
        align(div().flex().flex_col().gap_4(), locale)
            .child(identity)
            .child(resources)
            .child(
                columns
                    .gap_4()
                    .child(container_card(screen, data, loading))
                    .child(self.disk_card(screen, cx)),
            )
            .child(top_containers(
                screen,
                data.and_then(|data| data.utilization.as_ref()),
                loading,
                wide,
            ))
    }

    fn disk_card(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let disk = self.overview.disk.as_ref();
        let categories = [
            (
                Route::Images.name(locale),
                disk.and_then(|disk| disk.images),
                ColorToken::ChartCpu,
            ),
            (
                Route::Containers.name(locale),
                disk.and_then(|disk| disk.containers),
                ColorToken::ChartMemory,
            ),
            (
                Route::Volumes.name(locale),
                disk.and_then(|disk| disk.volumes),
                ColorToken::ChartNetwork,
            ),
            (
                gui::GUI_DESKTOP_BUILD_CACHE.text(locale),
                disk.and_then(|disk| disk.build_cache),
                ColorToken::ChartDisk,
            ),
        ];
        let max = categories.iter().filter_map(|(_, value, _)| *value).max();
        panel(screen)
            .flex_1()
            .min_w_0()
            .child(
                row(locale)
                    .justify_between()
                    .gap_2()
                    .child(heading(
                        screen,
                        gui::GUI_DESKTOP_OVERVIEW_DOCKER_DISK.text(locale),
                    ))
                    .child(
                        Button::new("overview-open-disk")
                            .ghost()
                            .label(gui::GUI_OVERVIEW_VIEW_DISK.text(locale))
                            .on_click(
                                cx.listener(|desktop, _, _, cx| desktop.navigate(Route::Disk, cx)),
                            ),
                    ),
            )
            .child(quiet(screen).child(if self.overview.disk_pending {
                gui::GUI_OVERVIEW_MEASURING.text(locale)
            } else if max.is_some() {
                gui::GUI_OVERVIEW_DISK_ACCOUNTING.text(locale)
            } else {
                gui::GUI_SYSTEM_DISK_BREAKDOWN_UNAVAILABLE.text(locale)
            }))
            .children(categories.map(|(label, value, token)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        row(locale)
                            .justify_between()
                            .gap_2()
                            .child(div().text_sm().child(label))
                            .child(mono(screen).child(value.map_or_else(
                                || technical("—"),
                                |bytes| technical(&disk_bytes(bytes)),
                            ))),
                    )
                    .children(value.zip(max).map(|(value, max)| {
                        let fraction = if max == 0 {
                            0.
                        } else {
                            (value as f64 / max as f64) as f32
                        };
                        row(locale)
                            .h(px(5.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(color(screen.palette, ColorToken::BgSubtle))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .bg(color(screen.palette, token)),
                            )
                    }))
            }))
    }
}

fn observed(sample: &OverviewSample) -> bool {
    sample.containers.is_some()
        || sample.version.is_some()
        || sample.images.is_some()
        || sample.volumes.is_some()
        || sample.networks.is_some()
}

fn container_card(screen: &ScreenContext<'_>, data: Option<&OverviewSample>, loading: bool) -> Div {
    let locale = screen.locale;
    let count = data.and_then(|data| data.containers.as_ref());
    let states = [
        (
            gui::GUI_DESKTOP_RUNNING,
            count.map(|count| count.running),
            ColorToken::StatusRunning,
        ),
        (
            gui::GUI_DESKTOP_STOPPED,
            count.map(|count| count.stopped),
            ColorToken::StatusStopped,
        ),
        (
            gui::GUI_DESKTOP_PAUSED,
            count.map(|count| count.paused),
            ColorToken::StatusPaused,
        ),
        (
            gui::GUI_OVERVIEW_RESTARTING,
            count.map(|count| count.restarting),
            ColorToken::StatusStarting,
        ),
        (
            gui::GUI_OVERVIEW_OTHER,
            count.map(|count| count.other),
            ColorToken::TextMuted,
        ),
    ];
    let health = [
        (
            gui::GUI_OVERVIEW_HEALTHY,
            count.map(|count| count.healthy),
            ColorToken::StatusRunning,
        ),
        (
            gui::GUI_OVERVIEW_UNHEALTHY,
            count.map(|count| count.unhealthy),
            ColorToken::StatusError,
        ),
        (
            gui::GUI_OVERVIEW_HEALTH_STARTING,
            count.map(|count| count.health_starting),
            ColorToken::StatusStarting,
        ),
        (
            gui::GUI_OVERVIEW_NO_HEALTH,
            count.map(|count| count.no_health),
            ColorToken::TextMuted,
        ),
        (
            gui::GUI_OVERVIEW_UNKNOWN_HEALTH,
            count.map(|count| count.unknown_health),
            ColorToken::StatusStopped,
        ),
    ];
    panel(screen)
        .flex_1()
        .min_w_0()
        .child(
            row(locale)
                .justify_between()
                .gap_2()
                .child(heading(screen, Route::Containers.name(locale)))
                .child(
                    screen
                        .typography(Role::Title)
                        .apply(div())
                        .child(number(count.map(|count| count.total))),
                ),
        )
        .children(count.filter(|count| count.total > 0).map(|count| {
            row(locale)
                .gap(px(2.))
                .h(px(6.))
                .w_full()
                .rounded_full()
                .overflow_hidden()
                .children(states.iter().filter_map(|(_, value, token)| {
                    let value = value.filter(|value| *value > 0)?;
                    Some(
                        div()
                            .h_full()
                            .flex_grow((value as f64 / count.total as f64) as f32)
                            .bg(color(screen.palette, *token)),
                    )
                }))
        }))
        .children(loading.then(|| quiet(screen).child(gui::GUI_OVERVIEW_MEASURING.text(locale))))
        .child(
            row(locale).gap_3().flex_wrap().children(
                states
                    .into_iter()
                    .filter(|(label, value, _)| {
                        *label != gui::GUI_OVERVIEW_OTHER || value.is_some_and(|value| value > 0)
                    })
                    .map(|(label, value, token)| count_badge(screen, label, value, token)),
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(color(screen.palette, ColorToken::BorderSubtle))
                .child(heading(screen, gui::GUI_OVERVIEW_HEALTH.text(locale)))
                .child(row(locale).gap_3().flex_wrap().children(
                    health.map(|(label, value, token)| count_badge(screen, label, value, token)),
                )),
        )
}

fn count_badge(
    screen: &ScreenContext<'_>,
    label: TextKey,
    value: Option<usize>,
    token: ColorToken,
) -> Div {
    row(screen.locale)
        .gap_2()
        .child(
            div()
                .size(px(6.))
                .flex_shrink_0()
                .rounded_full()
                .bg(color(screen.palette, token)),
        )
        .child(quiet(screen).child(label.text(screen.locale)))
        .child(mono(screen).child(number(value)))
}

fn top_containers(
    screen: &ScreenContext<'_>,
    data: Option<&OverviewUtilization>,
    loading: bool,
    wide: bool,
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
                by_cpu.len(),
                data.running,
                by_cpu.iter().filter_map(|entry| {
                    Some((entry.name.as_str(), format!("{:.1}%", entry.cpu_percent?)))
                }),
            ))
            .child(ranking(
                screen,
                gui::GUI_SYSTEM_MACHINE_MEMORY.text(locale),
                by_memory.len(),
                data.running,
                by_memory.iter().filter_map(|entry| {
                    Some((entry.name.as_str(), disk_bytes(entry.memory_bytes?)))
                }),
            )),
    );
    card
}

fn ranking<'a>(
    screen: &ScreenContext<'_>,
    label: &'static str,
    measured: usize,
    running: usize,
    entries: impl Iterator<Item = (&'a str, String)>,
) -> Div {
    let locale = screen.locale;
    align(div().flex_1().min_w_0().flex().flex_col().gap_2(), locale)
        .child(
            row(locale)
                .justify_between()
                .gap_2()
                .child(div().text_sm().child(label))
                .child(
                    row(locale)
                        .gap_2()
                        .child(quiet(screen).child(gui::GUI_OVERVIEW_SAMPLED.text(locale)))
                        .child(mono(screen).child(technical(&format!("{measured} / {running}")))),
                ),
        )
        .children(
            (measured == 0).then(|| quiet(screen).child(gui::GUI_DESKTOP_UNAVAILABLE.text(locale))),
        )
        .children(entries.take(4).map(|(name, usage)| {
            row(locale)
                .gap_3()
                .justify_between()
                .border_t_1()
                .pt_2()
                .border_color(color(screen.palette, ColorToken::BorderSubtle))
                .child(div().min_w_0().truncate().text_sm().child(technical(name)))
                .child(mono(screen).flex_shrink_0().child(technical(&usage)))
        }))
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
