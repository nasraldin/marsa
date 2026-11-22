//! Figma Overview layout backed by daemon and Docker snapshots.

use std::collections::VecDeque;

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, ParentElement, Styled, div, px, relative, svg},
    gpui_component::button::Button,
    gui_keys as gui,
};

use crate::{
    desktop::{Route, status},
    screens::common::{ScreenContext, align, color, panel, row, technical},
};

use super::{OpenRoute, SystemView, disk::bytes, machine::title_text};

impl SystemView {
    pub(super) fn overview(
        &self,
        screen: &ScreenContext<'_>,
        history: &VecDeque<EngineState>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let engine = screen.state.engine();
        let available = engine.is_some_and(|engine| {
            matches!(
                engine.state(),
                EngineState::Ready | EngineState::Idle | EngineState::Paused
            )
        });
        let unavailable = gui::GUI_DESKTOP_UNAVAILABLE.text(locale);
        let mut resources = panel(screen)
            .flex_1()
            .min_h(px(152.))
            .child(title_text(gui::GUI_DESKTOP_RESOURCES.text(locale)));
        let containers = if available {
            let containers = screen.state.containers();
            let running = containers
                .values()
                .filter(|container| container.state == "running")
                .count();
            format!("{running} / {}", containers.len())
        } else {
            "—".to_owned()
        };
        let projects = if available {
            screen.state.compose_projects().len().to_string()
        } else {
            "—".to_owned()
        };
        let usage = self.disk.as_ref().map(|disk| disk.host_allocated_bytes);
        let capacity = self
            .disk
            .as_ref()
            .map(|disk| disk.host_capacity_bytes)
            .filter(|capacity| *capacity > 0);
        let ratio = usage.zip(capacity).map_or(0., |(usage, capacity)| {
            (usage as f64 / capacity as f64).clamp(0., 1.) as f32
        });
        let disk_usage = match usage.zip(capacity) {
            Some((used, capacity)) => format!("{} / {}", bytes(used), bytes(capacity)),
            None => usage.map_or_else(|| unavailable.to_owned(), bytes),
        };

        for (icon, label, value) in [
            (
                "icons/box.svg",
                gui::GUI_DESKTOP_OVERVIEW_CONTAINER_USAGE,
                containers,
            ),
            (
                "icons/layout-template.svg",
                gui::GUI_DESKTOP_OVERVIEW_COMPOSE_PROJECTS,
                projects,
            ),
            (
                "icons/database.svg",
                gui::GUI_DESKTOP_OVERVIEW_DOCKER_DISK,
                usage.map_or_else(|| "—".to_owned(), bytes),
            ),
        ] {
            resources = resources.child(
                row(locale)
                    .justify_between()
                    .gap_3()
                    .child(
                        row(locale)
                            .gap_2()
                            .child(icon_view(screen, icon))
                            .child(label.text(locale)),
                    )
                    .child(quiet(screen).child(technical(&value))),
            );
        }

        let virtual_machine = self
            .settings
            .as_ref()
            .map(|(settings, _)| {
                let memory = self
                    .vm_stats
                    .as_ref()
                    .map(|stats| stats.memory_total_bytes)
                    .filter(|memory| *memory > 0)
                    .or(settings.vm.memory);
                let cpu = settings.vm.cpus;
                memory.map_or_else(
                    || format!("{cpu} vCPU"),
                    |memory| format!("{cpu} vCPU · {}", bytes(memory)),
                )
            })
            .unwrap_or_else(|| "—".to_owned());

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                panel(screen)
                    .gap_4()
                    .child(
                        row(locale)
                            .justify_between()
                            .child(title_text(gui::GUI_DESKTOP_LOCAL_ENGINE.text(locale)))
                            .child(status::compact_badge(
                                locale,
                                screen.palette,
                                engine.map(|engine| engine.state()),
                            )),
                    )
                    .child(
                        row(locale)
                            .gap_3()
                            .child(summary_field(
                                screen,
                                gui::GUI_DESKTOP_DOCKER_CONTEXT.text(locale),
                                "marsa",
                            ))
                            .child(summary_field(
                                screen,
                                gui::GUI_DESKTOP_OVERVIEW_ARCHITECTURE.text(locale),
                                "arm64",
                            ))
                            .child(summary_field(
                                screen,
                                gui::GUI_DESKTOP_OVERVIEW_RUNTIME.text(locale),
                                "Docker Engine",
                            ))
                            .child(summary_field(
                                screen,
                                gui::GUI_DESKTOP_OVERVIEW_VIRTUAL_MACHINE.text(locale),
                                &virtual_machine,
                            )),
                    ),
            )
            .child(
                row(locale).items_start().gap_4().child(resources).child(
                    panel(screen)
                        .flex_1()
                        .min_h(px(152.))
                        .child(title_text(gui::GUI_DESKTOP_DISK_USAGE.text(locale)))
                        .child(
                            quiet(screen)
                                .child(gui::GUI_DESKTOP_OVERVIEW_DISK_DESCRIPTION.text(locale)),
                        )
                        .child(
                            row(locale)
                                .h(px(8.))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(color(screen.palette, ColorToken::BgSubtle))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(ratio))
                                        .rounded_full()
                                        .bg(color(screen.palette, ColorToken::AccentDefault)),
                                ),
                        )
                        .child(
                            row(locale)
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family("IBM Plex Mono")
                                        .child(technical(&disk_usage)),
                                )
                                .child(
                                    Button::new("overview-view-disk")
                                        .outline()
                                        .label(gui::GUI_DESKTOP_OVERVIEW_VIEW_DISK.text(locale))
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            cx.emit(OpenRoute(Route::Disk))
                                        })),
                                ),
                        ),
                ),
            )
            .child(
                panel(screen)
                    .gap_3()
                    .min_h(px(144.))
                    .child(title_text(gui::GUI_DESKTOP_RECENT_ACTIVITY.text(locale)))
                    .children(history.is_empty().then(|| {
                        quiet(screen).child(gui::GUI_DESKTOP_NO_EVENTS_IN_THIS_SESSION.text(locale))
                    }))
                    .children(history.iter().rev().take(4).map(|state| {
                        row(locale)
                            .gap_2()
                            .h(px(40.))
                            .border_t_1()
                            .border_color(color(screen.palette, ColorToken::BorderSubtle))
                            .child(icon_view(screen, "icons/activity.svg"))
                            .child(status::text(locale, Some(*state)))
                    })),
            )
    }
}

fn icon_view(screen: &ScreenContext<'_>, name: &'static str) -> marsa_ui::gpui::Svg {
    svg()
        .path(name)
        .size(px(18.))
        .text_color(color(screen.palette, ColorToken::TextMuted))
}

fn quiet(screen: &ScreenContext<'_>) -> Div {
    align(
        div()
            .text_xs()
            .text_color(color(screen.palette, ColorToken::TextMuted)),
        screen.locale,
    )
}

fn summary_field(screen: &ScreenContext<'_>, title: &'static str, value: &str) -> Div {
    align(
        div().flex().flex_1().min_w_0().flex_col().gap_1(),
        screen.locale,
    )
    .child(quiet(screen).child(title))
    .child(
        div()
            .text_xs()
            .font_family("IBM Plex Mono")
            .child(technical(value)),
    )
}
