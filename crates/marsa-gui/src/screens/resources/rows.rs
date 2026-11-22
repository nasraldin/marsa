//! Normalize engine resources into the shared Foundations table layout.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::time::{SystemTime, UNIX_EPOCH};

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Div, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
        StatefulInteractiveElement, Styled, div, px,
    },
    tokens::{
        scale::{Radius, Space},
        typography::Role,
    },
};

use crate::screens::common::{ScreenContext, copy, label};

use super::{ResourceKind, ResourcesView, forms::FormKind, logical_row, text, tone};

pub(super) struct Columns(Vec<(SharedString, f32)>);

impl Columns {
    pub(super) fn for_kind(screen: &ScreenContext<'_>, kind: ResourceKind) -> Self {
        let fields: &[(&str, &str, f32)] = match kind {
            ResourceKind::Images => &[
                ("Repository", "المستودع", 0.),
                ("Tag", "الوسم", 144.),
                ("Size", "الحجم", 96.),
                ("Age", "العمر", 96.),
                ("State", "الحالة", 112.),
            ],
            ResourceKind::Volumes => &[
                ("Name", "الاسم", 0.),
                ("Driver", "برنامج التشغيل", 144.),
                ("Size", "الحجم", 96.),
                ("Containers", "الحاويات", 200.),
            ],
            ResourceKind::Networks => &[
                ("Name", "الاسم", 0.),
                ("Driver", "برنامج التشغيل", 144.),
                ("Scope", "النطاق", 96.),
                ("Containers", "الحاويات", 200.),
            ],
            ResourceKind::Compose => &[
                ("Project", "المشروع", 0.),
                ("Services", "الخدمات", 168.),
                ("Running", "قيد التشغيل", 112.),
                ("Containers", "الحاويات", 112.),
            ],
        };
        Self(
            fields
                .iter()
                .map(|&(en, ar, width)| (label(screen, en, ar), width))
                .collect(),
        )
    }
}

pub(super) struct ResourceRow {
    pub id: String,
    key: String,
    cells: Vec<String>,
    pub details: Vec<(SharedString, String)>,
    status: Option<(ColorToken, ColorToken)>,
    action_target: String,
    users: Vec<String>,
    references: Option<u64>,
    pub members: Vec<(marsa_client::docker::ContainerId, String)>,
}

impl ResourceRow {
    pub(super) fn selected(&self, key: &str) -> bool {
        self.key == key
    }

    pub(super) fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.id.to_lowercase().contains(query)
            || self
                .cells
                .iter()
                .any(|value| value.to_lowercase().contains(query))
            || self
                .details
                .iter()
                .any(|(_, value)| value.to_lowercase().contains(query))
    }

    pub(super) fn actions(&self, kind: ResourceKind) -> Vec<FormKind> {
        match kind {
            ResourceKind::Images => {
                let mut actions = vec![FormKind::RemoveImage {
                    id: self.id.clone(),
                    reference: self.action_target.clone(),
                    users: self.users.clone(),
                }];
                if !self.action_target.is_empty() {
                    actions.insert(
                        0,
                        FormKind::Run {
                            id: self.id.clone(),
                            reference: self.action_target.clone(),
                        },
                    );
                }
                actions
            }
            ResourceKind::Volumes => vec![FormKind::RemoveVolume {
                name: self.id.clone(),
                users: self.users.clone(),
                in_use: self.references.is_some_and(|count| count != 0),
            }],
            ResourceKind::Networks => vec![FormKind::RemoveNetwork {
                id: self.id.clone(),
                name: self.action_target.clone(),
                users: self.users.clone(),
            }],
            ResourceKind::Compose => vec![
                FormKind::ComposeUp {
                    project: self.id.clone(),
                },
                FormKind::ComposeRestart {
                    project: self.id.clone(),
                },
                FormKind::ComposeDown {
                    project: self.id.clone(),
                    volumes: false,
                },
                FormKind::ComposeDown {
                    project: self.id.clone(),
                    volumes: true,
                },
            ],
        }
    }
}

pub(super) fn table(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    columns: &Columns,
    rows: &[ResourceRow],
    view: &Entity<ResourcesView>,
    selection: Option<&(ResourceKind, String)>,
) -> Div {
    let border = tone(screen, ColorToken::BorderSubtle);
    let header = logical_row(screen.locale)
        .items_center()
        .h(px(Space::S36.pixels()))
        .px(px(Space::S16.pixels()))
        .gap(px(Space::S16.pixels()))
        .bg(tone(screen, ColorToken::BgSubtle))
        .border_b_1()
        .border_color(border)
        .children(columns.0.iter().map(|(title, width)| {
            column(
                *width,
                text(screen, Role::Caption, title.clone())
                    .text_color(tone(screen, ColorToken::TextMuted)),
            )
        }));
    let mut table = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(Radius::R12.pixels()))
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .bg(tone(screen, ColorToken::BgElevated))
        .child(header);
    if rows.is_empty() {
        let engine_ready = screen
            .state
            .engine()
            .is_some_and(|engine| matches!(engine.state(), EngineState::Ready | EngineState::Idle));
        let message = if engine_ready {
            message(screen.locale, gui::GUI_DESKTOP_NO_MATCHING_RESOURCES)
        } else {
            message(
                screen.locale,
                gui::GUI_DESKTOP_RESOURCES_ARE_AVAILABLE_WHEN_THE_ENGINE_IS_RUNNING,
            )
        };
        return table.child(
            text(screen, Role::Body, message)
                .text_color(tone(screen, ColorToken::TextMuted))
                .p(px(Space::S16.pixels())),
        );
    }
    table = table.children(rows.iter().map(|row| {
        let selected = selection.is_some_and(|(page, key)| *page == kind && key == &row.key);
        let id = row.key.clone();
        let entry = logical_row(screen.locale)
            .items_center()
            .min_h(px(Space::S40.pixels() + Space::S4.pixels()))
            .px(px(Space::S16.pixels()))
            .gap(px(Space::S16.pixels()))
            .border_b_1()
            .border_color(border);
        let entry = if selected {
            entry.bg(tone(screen, ColorToken::AccentSubtle))
        } else {
            entry
        };
        let select = view.clone();
        let click_key = id.clone();
        let click_id = row.id.clone();
        let click_service = screen.services.cloned();
        let key_select = view.clone();
        let key_id = row.id.clone();
        let key_service = screen.services.cloned();
        entry
            .id(SharedString::from(id.clone()))
            .tab_stop(true)
            .cursor_pointer()
            .hover(|style| style.bg(tone(screen, ColorToken::BgHover)))
            .on_click(move |_, _, cx| {
                select.update(cx, |view, cx| {
                    view.select(
                        kind,
                        click_key.clone(),
                        &click_id,
                        click_service.as_deref(),
                        cx,
                    );
                });
            })
            .on_key_down(move |event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    key_select.update(cx, |view, cx| {
                        view.select(kind, id.clone(), &key_id, key_service.as_deref(), cx);
                    });
                    cx.stop_propagation();
                }
            })
            .children(columns.0.iter().zip(&row.cells).enumerate().map(
                |(index, ((_, width), value))| {
                    if let Some((foreground, background)) = row.status.filter(|_| index == 4) {
                        return column(
                            *width,
                            text(screen, Role::Small, value.clone())
                                .rounded(px(Radius::Full.pixels()))
                                .px(px(Space::S8.pixels()))
                                .py(px(Space::S4.pixels()))
                                .text_color(tone(screen, foreground))
                                .bg(tone(screen, background)),
                        )
                        .into_any_element();
                    }
                    column(*width, text(screen, Role::Mono, value.clone()).truncate())
                        .into_any_element()
                },
            ))
    }));
    table
}

fn column(width: f32, cell: Div) -> Div {
    let wrapper = div().min_w_0().overflow_hidden().child(cell);
    if width == 0. {
        wrapper.flex_1()
    } else {
        wrapper.w(px(width)).flex_shrink_0()
    }
}

pub(super) fn snapshot_rows(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    volume_usage: &std::collections::BTreeMap<String, marsa_client::docker::VolumeUsage>,
) -> Vec<ResourceRow> {
    let containers = screen.state.containers();
    let missing = gui::GUI_DESKTOP_UNAVAILABLE.text(screen.locale);
    match kind {
        ResourceKind::Images => screen
            .state
            .images()
            .values()
            .flat_map(|image| {
                let users = containers
                    .values()
                    .filter(|container| container.image_id == image.id)
                    .map(container_name)
                    .collect::<Vec<_>>();
                let usage = if users.is_empty() {
                    gui::GUI_DESKTOP_UNUSED.text(screen.locale)
                } else {
                    gui::GUI_DESKTOP_IN_USE.text(screen.locale)
                };
                let status = if users.is_empty() {
                    (ColorToken::StatusStopped, ColorToken::StatusStoppedBg)
                } else {
                    (ColorToken::StatusRunning, ColorToken::StatusRunningBg)
                };
                let refs = if image.repo_tags.is_empty() {
                    vec![None]
                } else {
                    image
                        .repo_tags
                        .iter()
                        .map(|tag| (tag != "<none>:<none>").then_some(tag.as_str()))
                        .collect()
                };
                refs.into_iter()
                    .map(|reference| {
                        let (repository, tag) = split_reference(reference);
                        let repository = repository
                            .unwrap_or_else(|| gui::GUI_DESKTOP_UNTAGGED.text(screen.locale));
                        let size = format_bytes(screen.locale, image.size);
                        let age = format_age(screen.locale, image.created)
                            .unwrap_or_else(|| missing.to_owned());
                        let reference = reference.unwrap_or(&image.id);
                        ResourceRow {
                            id: image.id.to_owned(),
                            key: format!("{}:{reference}", image.id),
                            cells: vec![
                                repository.to_owned(),
                                tag.unwrap_or(missing).to_owned(),
                                size.clone(),
                                age.clone(),
                                usage.to_owned(),
                            ],
                            details: vec![
                                (
                                    message(screen.locale, gui::GUI_DESKTOP_REFERENCE),
                                    reference.to_owned(),
                                ),
                                (message(screen.locale, gui::GUI_DESKTOP_SIZE), size),
                                (message(screen.locale, gui::GUI_DESKTOP_AGE), age),
                                (
                                    message(screen.locale, gui::GUI_DESKTOP_CONTAINERS),
                                    joined(screen.locale, &users),
                                ),
                            ],
                            status: Some(status),
                            action_target: reference.to_owned(),
                            users: users.clone(),
                            references: None,
                            members: Vec::new(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
        ResourceKind::Volumes => screen
            .state
            .volumes()
            .values()
            .map(|volume| {
                let users = containers
                    .values()
                    .filter(|container| {
                        container.mounts.iter().any(|mount| {
                            mount.kind == "volume"
                                && mount.name.as_deref() == Some(volume.name.as_str())
                        })
                    })
                    .map(container_name)
                    .collect::<Vec<_>>();
                let usage = volume_usage.get(&volume.name);
                let size = usage.and_then(|usage| usage.size_bytes).map_or_else(
                    || missing.to_owned(),
                    |size| format_bytes(screen.locale, size),
                );
                let attached = if users.is_empty()
                    && usage
                        .and_then(|usage| usage.references)
                        .is_some_and(|count| count != 0)
                {
                    gui::GUI_DESKTOP_IN_USE.text(screen.locale).to_owned()
                } else {
                    joined(screen.locale, &users)
                };
                ResourceRow {
                    id: volume.name.clone(),
                    key: volume.name.clone(),
                    cells: vec![
                        volume.name.clone(),
                        volume.driver.clone(),
                        size.clone(),
                        attached.clone(),
                    ],
                    details: vec![
                        (
                            message(screen.locale, gui::GUI_DESKTOP_DRIVER),
                            volume.driver.clone(),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_SCOPE),
                            volume.scope.clone(),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_CREATED),
                            volume
                                .created_at
                                .clone()
                                .unwrap_or_else(|| missing.to_owned()),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_CONTAINERS),
                            attached,
                        ),
                    ],
                    status: None,
                    action_target: volume.name.clone(),
                    users,
                    references: usage.and_then(|usage| usage.references),
                    members: Vec::new(),
                }
            })
            .collect(),
        ResourceKind::Networks => screen
            .state
            .networks()
            .values()
            .map(|network| {
                let users = containers
                    .values()
                    .filter(|container| {
                        container.network_settings.as_ref().is_some_and(|settings| {
                            settings
                                .networks
                                .values()
                                .any(|endpoint| endpoint.network_id == network.id)
                        })
                    })
                    .map(container_name)
                    .collect::<Vec<_>>();
                let attached = joined(screen.locale, &users);
                ResourceRow {
                    id: network.id.clone(),
                    key: network.id.clone(),
                    cells: vec![
                        network.name.clone(),
                        network.driver.clone(),
                        network.scope.clone(),
                        attached.clone(),
                    ],
                    details: vec![
                        (
                            message(screen.locale, gui::GUI_DESKTOP_NAME),
                            network.name.clone(),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_DRIVER),
                            network.driver.clone(),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_SCOPE),
                            network.scope.clone(),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_INTERNAL),
                            if network.internal {
                                gui::GUI_DESKTOP_YES.text(screen.locale)
                            } else {
                                gui::GUI_DESKTOP_NO.text(screen.locale)
                            }
                            .to_owned(),
                        ),
                        (
                            message(screen.locale, gui::GUI_DESKTOP_CONTAINERS),
                            attached,
                        ),
                    ],
                    status: None,
                    action_target: network.name.clone(),
                    users,
                    references: None,
                    members: Vec::new(),
                }
            })
            .collect(),
        ResourceKind::Compose => screen
            .state
            .compose_projects()
            .values()
            .map(|project| {
                let members = project
                    .containers
                    .iter()
                    .filter_map(|id| containers.get(id))
                    .collect::<Vec<_>>();
                let services = members
                    .iter()
                    .filter_map(|container| container.compose_service())
                    .collect::<std::collections::BTreeSet<_>>();
                let running = members
                    .iter()
                    .filter(|container| container.state == "running")
                    .count();
                ResourceRow {
                    id: project.name.clone(),
                    key: project.name.clone(),
                    cells: vec![
                        project.name.clone(),
                        services.len().to_string(),
                        running.to_string(),
                        members.len().to_string(),
                    ],
                    details: members
                        .iter()
                        .map(|container| {
                            (
                                SharedString::from(
                                    container
                                        .compose_service()
                                        .unwrap_or_else(|| container.id.as_str())
                                        .to_owned(),
                                ),
                                container.status.clone(),
                            )
                        })
                        .collect(),
                    status: None,
                    action_target: project.name.clone(),
                    users: Vec::new(),
                    references: None,
                    members: members
                        .iter()
                        .map(|container| {
                            (
                                container.id.clone(),
                                container
                                    .compose_service()
                                    .unwrap_or_else(|| container.id.as_str())
                                    .to_owned(),
                            )
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

fn container_name(container: &marsa_client::docker::ContainerSummary) -> String {
    container
        .names
        .first()
        .map(|name| name.trim_start_matches('/'))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| container.id.as_str())
        .to_owned()
}

fn split_reference(reference: Option<&str>) -> (Option<&str>, Option<&str>) {
    let Some(reference) = reference else {
        return (None, None);
    };
    if let Some((repository, digest)) = reference.rsplit_once('@') {
        return (Some(repository), Some(digest));
    }
    if let Some((repository, tag)) = reference.rsplit_once(':') {
        if !tag.contains('/') {
            return (Some(repository), Some(tag));
        }
    }
    (Some(reference), None)
}

fn joined(locale: Locale, values: &[String]) -> String {
    if values.is_empty() {
        gui::GUI_DESKTOP_NONE.text(locale).to_owned()
    } else {
        values.join(copy(locale, ", ", "، "))
    }
}

fn format_bytes(locale: Locale, bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!(
            "{:.1} {}",
            bytes as f64 / 1_000_000_000.,
            gui::GUI_DESKTOP_GB.text(locale)
        )
    } else if bytes >= 1_000_000 {
        format!(
            "{:.1} {}",
            bytes as f64 / 1_000_000.,
            gui::GUI_DESKTOP_MB.text(locale)
        )
    } else if bytes >= 1_000 {
        format!(
            "{:.1} {}",
            bytes as f64 / 1_000.,
            gui::GUI_DESKTOP_KB.text(locale)
        )
    } else {
        format!("{bytes} {}", gui::GUI_DESKTOP_B.text(locale))
    }
}

fn format_age(locale: Locale, created: i64) -> Option<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let age = now.checked_sub(u64::try_from(created).ok()?)?;
    let (value, unit) = if age >= 86_400 {
        (age / 86_400, gui::GUI_DESKTOP_D.text(locale))
    } else if age >= 3_600 {
        (age / 3_600, gui::GUI_DESKTOP_H.text(locale))
    } else if age >= 60 {
        (age / 60, gui::GUI_DESKTOP_MIN.text(locale))
    } else {
        (age, gui::GUI_DESKTOP_S.text(locale))
    };
    Some(format!("{value} {unit}"))
}
