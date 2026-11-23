//! A single inventory of Marsa, saved connections, and detected Docker engines.

use marsa_ui::gpui_component::scroll::ScrollableElement as _;

use std::{iter, sync::Arc};

use marsa_client::{
    api::{EngineHealthState, EngineState, EngineStatusRequest},
    docker::{ManagedEndpointTransport, VerifiedEngine},
};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, InteractiveElement as _, ParentElement as _, SharedString,
        StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
    },
    gpui_component::{
        Disableable as _, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        tooltip::Tooltip,
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{
    EditorTarget, SystemView, Update, confirm_lima_stop, editor::Appearance, feedback_text,
    settings_text, vm::lima_next_action,
};
use crate::{
    screens::{
        common::{ScreenContext, color, row, technical},
        external_inventory::Environment,
        system::SelectContext,
    },
    services::{EngineAction, Pending, Services},
};

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum ConnectionKey {
    Builtin,
    Saved(String),
    Discovered { source: String, address: String },
}

pub(super) enum ConnectionCheck {
    Connected(VerifiedEngine),
    Local(EngineState, EngineHealthState),
    Failed,
}

impl ConnectionCheck {
    pub(super) fn passed(&self) -> bool {
        matches!(
            self,
            Self::Connected(_)
                | Self::Local(
                    EngineState::Ready | EngineState::Idle,
                    EngineHealthState::Ready
                )
        )
    }
}

pub(super) fn check_builtin_connection(services: &Services) -> Pending<ConnectionCheck> {
    // The Docker socket starts the VM on use. Status only queries the daemon.
    services.request(|client| async move {
        let status = client
            .engine()
            .status(EngineStatusRequest {})
            .await?
            .into_inner();
        Ok(ConnectionCheck::Local(
            status.state(),
            status.guest_health(),
        ))
    })
}

impl SystemView {
    fn environment_rows(&self) -> Vec<EditorTarget> {
        let saved =
            self.environment_settings
                .saved
                .iter()
                .flatten()
                .map(|entry| {
                    let workspace = self.external_inventory.iter().find(|workspace| {
                        workspace.endpoint.managed_id() == Some(entry.id.as_str())
                    });
                    EditorTarget::Saved(Box::new(entry.clone()), workspace.cloned())
                });
        let discovered = self
            .external_inventory
            .iter()
            .filter(|workspace| workspace.endpoint.managed_id().is_none())
            .map(|workspace| EditorTarget::Discovered(Arc::clone(workspace)));
        iter::once(EditorTarget::Builtin)
            .chain(saved)
            .chain(discovered)
            .collect()
    }

    fn check_environment_rows(&mut self, entries: Vec<EditorTarget>, cx: &mut Context<Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        if self.environment_settings.task.is_some() || entries.is_empty() {
            return;
        }
        self.environment_settings.checking = true;
        self.update_environments(
            async move {
                let mut entries = entries.into_iter();
                let mut results = Vec::new();
                loop {
                    let batch: Vec<_> = entries
                        .by_ref()
                        .take(8)
                        .map(|entry| {
                            let key = entry.key();
                            let result = match entry {
                                EditorTarget::Builtin => check_builtin_connection(&services),
                                EditorTarget::Saved(endpoint, _) => services.pending(async move {
                                    Ok(endpoint.config.test().await.map_or(
                                        ConnectionCheck::Failed,
                                        ConnectionCheck::Connected,
                                    ))
                                }),
                                EditorTarget::Discovered(workspace) => {
                                    services.pending(async move {
                                        Ok(workspace.endpoint.test_connection().await.map_or(
                                            ConnectionCheck::Failed,
                                            ConnectionCheck::Connected,
                                        ))
                                    })
                                }
                                EditorTarget::Add => {
                                    services.pending(async { Ok(ConnectionCheck::Failed) })
                                }
                            };
                            (key, result)
                        })
                        .collect();
                    if batch.is_empty() {
                        break;
                    }
                    for (key, mut result) in batch {
                        results.push((
                            key,
                            result
                                .result()
                                .await
                                .and_then(Result::ok)
                                .unwrap_or(ConnectionCheck::Failed),
                        ));
                    }
                }
                Ok(Update::Checked(results))
            },
            None,
            cx,
        );
    }

    pub(in crate::screens::system::settings) fn external_docker(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let panel = &self.environment_settings;
        let discovering = self.external_inventory_task.is_some();
        let disabled = panel.task.is_some() || panel.editor.is_some();
        let appearance = Appearance::from(screen);
        let targets = self.environment_rows();
        let controls = row(locale)
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("add-environment")
                    .primary()
                    .icon(IconName::Plus)
                    .label(gui::GUI_ENVIRONMENT_ADD.text(locale))
                    .disabled(disabled || self.services.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_environment_editor(
                            EditorTarget::Add,
                            appearance.clone(),
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                Button::new("test-all-environments")
                    .outline()
                    .label(gui::GUI_ENVIRONMENT_TEST_ALL.text(locale))
                    .disabled(disabled || discovering || self.services.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.check_environment_rows(this.environment_rows(), cx);
                    })),
            )
            .child(
                Button::new("refresh-environments")
                    .outline()
                    .label(gui::GUI_DESKTOP_REFRESH.text(locale))
                    .disabled(disabled || discovering || self.services.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_environments(cx))),
            );
        let boundary = color(screen.palette, ColorToken::BorderSubtle);
        let headings = [
            gui::GUI_DESKTOP_NAME,
            gui::GUI_ENVIRONMENT_TABLE_CONNECTION,
            gui::GUI_ENVIRONMENT_LABELS,
            gui::GUI_ENVIRONMENT_TABLE_STATUS,
            gui::GUI_ENVIRONMENT_TABLE_DOCKER,
            gui::GUI_PALETTE_ACTIONS,
        ]
        .map(|label| {
            settings_text(
                screen,
                Role::Small,
                label.text(locale),
                ColorToken::TextMuted,
            )
        });
        let table = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(710.))
            .rounded(px(12.))
            .border_1()
            .border_color(boundary)
            .overflow_hidden()
            .bg(color(screen.palette, ColorToken::BgElevated))
            .child(
                table_line(locale, headings)
                    .min_h(px(40.))
                    .bg(color(screen.palette, ColorToken::BgSubtle)),
            )
            .children(
                targets
                    .iter()
                    .enumerate()
                    .map(|(index, target)| self.environment_table_row(screen, target, index, cx)),
            );

        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                row(locale)
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .flex_wrap()
                    .child(settings_text(
                        screen,
                        Role::Label,
                        format!(
                            "{}  {}",
                            gui::GUI_ENVIRONMENT_TABLE_TOTAL.text(locale),
                            targets.len()
                        ),
                        ColorToken::TextPrimary,
                    ))
                    .child(controls),
            )
            .children((panel.task.is_some() || discovering).then(|| {
                let status = if panel.checking {
                    gui::GUI_ENVIRONMENT_CHECKING
                } else {
                    gui::GUI_DESKTOP_WORKING
                };
                settings_text(
                    screen,
                    Role::Small,
                    status.text(locale),
                    ColorToken::TextMuted,
                )
            }))
            .children(
                panel
                    .feedback
                    .as_ref()
                    .map(|feedback| feedback_text(screen, feedback)),
            )
            .child(
                div()
                    .id("environment-table-scroll")
                    .w_full()
                    .min_w_0()
                    .relative()
                    .child(
                        div()
                            .id("environment-table-viewport")
                            .w_full()
                            .min_w_0()
                            .track_scroll(&panel.table_scroll)
                            .overflow_x_scroll()
                            .child(table),
                    )
                    .horizontal_scrollbar(&panel.table_scroll),
            )
    }

    fn environment_table_row(
        &self,
        screen: &ScreenContext<'_>,
        target: &EditorTarget,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let panel = &self.environment_settings;
        let workspace = target.workspace();
        let lima = workspace.and_then(|workspace| {
            Some((
                workspace.endpoint.lima_instance()?,
                workspace.lima_settings?,
            ))
        });
        let check = panel.checks.get(&target.key());
        let disabled = panel.task.is_some() || panel.editor.is_some();
        let (origin, address, labels) = match target {
            EditorTarget::Builtin => (
                gui::GUI_ENVIRONMENT_TABLE_BUILTIN,
                marsa_client::docker_socket_path()
                    .ok()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                &[][..],
            ),
            EditorTarget::Saved(entry, _) => (
                gui::GUI_ENVIRONMENT_TABLE_SAVED,
                match &entry.config.transport {
                    ManagedEndpointTransport::Unix { socket } => socket.display().to_string(),
                    ManagedEndpointTransport::Ssh { address }
                    | ManagedEndpointTransport::Tls { address, .. }
                    | ManagedEndpointTransport::HawserStandard { address, .. } => address.clone(),
                    ManagedEndpointTransport::HawserEdge { bind, agent_id, .. } => {
                        format!(
                            "{agent_id} · wss://{bind}{}",
                            marsa_client::docker::HAWSER_EDGE_PATH
                        )
                    }
                },
                entry.config.labels.as_slice(),
            ),
            EditorTarget::Discovered(workspace) => (
                gui::GUI_ENVIRONMENT_TABLE_DISCOVERED,
                workspace.endpoint.address(),
                &[][..],
            ),
            EditorTarget::Add => return div(),
        };
        let (name, chosen_icon) = self.editor_presentation(target);
        let label = row(locale)
            .gap_2()
            .items_center()
            .child(
                marsa_ui::gpui::svg()
                    .path(super::icon_path(chosen_icon))
                    .size(px(18.))
                    .flex_shrink_0()
                    .text_color(color(screen.palette, ColorToken::AccentDefault)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap_1()
                    .child(
                        settings_text(
                            screen,
                            Role::Label,
                            technical(&name),
                            ColorToken::TextPrimary,
                        )
                        .truncate(),
                    )
                    .child(settings_text(
                        screen,
                        Role::Small,
                        origin.text(locale),
                        ColorToken::TextMuted,
                    )),
            );
        let connection = if address.is_empty() {
            "—".into()
        } else {
            technical(&address)
        };
        let tooltip = connection.clone();
        let connection = div().child(
            settings_text(screen, Role::Small, connection, ColorToken::TextMuted)
                .truncate()
                .id(("environment-address", index))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)),
        );
        let labels = if labels.is_empty() {
            settings_text(screen, Role::Small, "—", ColorToken::TextMuted)
        } else {
            row(locale)
                .flex_wrap()
                .gap_1()
                .children(labels.iter().map(|label| {
                    settings_text(
                        screen,
                        Role::Small,
                        technical(label),
                        ColorToken::TextPrimary,
                    )
                    .rounded_full()
                    .px_2()
                    .py_1()
                    .max_w_full()
                    .truncate()
                    .bg(color(screen.palette, ColorToken::AccentSubtle))
                }))
        };
        let verified = match check {
            Some(ConnectionCheck::Connected(verified)) => Some(verified),
            Some(ConnectionCheck::Failed) => None,
            _ => workspace.and_then(|workspace| workspace.verified_engine.as_ref()),
        };
        let engine_version = if matches!(target, EditorTarget::Builtin) {
            self.version
                .as_ref()
                .and_then(|version| {
                    version
                        .components
                        .iter()
                        .find(|component| component.name == "docker-engine")
                })
                .map(|component| component.version.as_str())
        } else {
            verified.map(|verified| verified.version.as_str())
        };
        let version: SharedString = engine_version.map_or_else(|| "—".into(), technical);
        let status = match check {
            Some(check) if check.passed() => status_line(
                screen,
                gui::GUI_ENVIRONMENT_CONNECTED,
                ColorToken::StatusRunning,
            ),
            Some(ConnectionCheck::Local(state, _))
                if !matches!(state, EngineState::Ready | EngineState::Idle) =>
            {
                crate::desktop::status::compact_badge(locale, screen.palette, Some(*state))
            }
            Some(_) => status_line(
                screen,
                gui::GUI_ENVIRONMENT_TEST_UNREACHABLE,
                ColorToken::StatusWarning,
            ),
            None if matches!(target, EditorTarget::Builtin) => {
                crate::desktop::status::compact_badge(
                    locale,
                    screen.palette,
                    screen.state.engine().map(|engine| engine.state()),
                )
            }
            None if lima.as_ref().is_some_and(|(_, settings)| settings.stopped) => {
                crate::desktop::status::compact_badge(
                    locale,
                    screen.palette,
                    Some(EngineState::Stopped),
                )
            }
            None => {
                let (label, color) = match workspace {
                    Some(workspace) if workspace.reachable => {
                        (gui::GUI_ENVIRONMENT_CONNECTED, ColorToken::StatusRunning)
                    }
                    Some(_) => (gui::GUI_DESKTOP_UNAVAILABLE, ColorToken::TextMuted),
                    None => (gui::GUI_ENVIRONMENT_UNCHECKED, ColorToken::TextMuted),
                };
                status_line(screen, label, color)
            }
        };

        let appearance = Appearance::from(screen);
        let edit = target.clone();
        let test = target.clone();
        let selection = match target {
            EditorTarget::Builtin => Some(Environment::Marsa),
            _ => workspace
                .filter(|workspace| {
                    workspace.endpoint.managed_id().is_some()
                        || !workspace.endpoint.address().is_empty()
                })
                .map(Environment::from),
        };
        let lifecycle = if matches!(target, EditorTarget::Builtin) {
            match screen.state.engine().map(|engine| engine.state()) {
                Some(EngineState::Ready | EngineState::Idle | EngineState::Paused) => Some((
                    EngineAction::Stop,
                    IconName::Square,
                    gui::GUI_DESKTOP_STOP_MACHINE,
                )),
                Some(EngineState::Stopped | EngineState::Failed) => Some((
                    EngineAction::Start,
                    IconName::Play,
                    gui::GUI_DESKTOP_START_MACHINE,
                )),
                _ => None,
            }
        } else {
            None
        };
        let actions = row(locale)
            .gap_1()
            .items_center()
            .children(lifecycle.map(|(action, icon, label)| {
                row_action(
                    "environment-engine-lifecycle",
                    index,
                    icon,
                    label.text(locale),
                )
                .disabled(disabled || self.busy())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.engine(action, locale, palette, window, cx)
                }))
            }))
            .children(lima.and_then(|(instance, settings)| {
                let (running, icon, label) = lima_next_action(settings)?;
                Some(
                    row_action(
                        "environment-lima-lifecycle",
                        index,
                        icon,
                        label.text(locale),
                    )
                    .disabled(disabled || self.services.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let instance = instance.clone();
                        if running {
                            this.change_lima_row_state(instance, true, cx);
                        } else {
                            let name = instance.name().to_owned();
                            confirm_lima_stop(&name, locale, window, cx, move |this, _, cx| {
                                this.change_lima_row_state(instance, false, cx);
                            });
                        }
                    })),
                )
            }))
            .child(
                row_action(
                    "use-environment",
                    index,
                    IconName::ArrowUp,
                    gui::GUI_ENVIRONMENT_USE.text(locale),
                )
                .disabled(disabled || selection.is_none())
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(selection) = &selection {
                        cx.emit(SelectContext(selection.clone()));
                    }
                })),
            )
            .child(
                row_action(
                    "test-environment-row",
                    index,
                    IconName::Network,
                    gui::GUI_ENVIRONMENT_TEST.text(locale),
                )
                .disabled(disabled || self.services.is_none())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.check_environment_rows(vec![test.clone()], cx)
                })),
            )
            .child(
                row_action(
                    "edit-environment",
                    index,
                    IconName::Settings2,
                    gui::GUI_ENVIRONMENT_EDIT_TITLE.text(locale),
                )
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_environment_editor(edit.clone(), appearance.clone(), window, cx)
                })),
            )
            .children(match target {
                EditorTarget::Saved(entry, _) => {
                    let id = entry.id.clone();
                    Some(
                        Button::new(("remove-environment", index))
                            .ghost()
                            .small()
                            .icon(marsa_ui::gpui_component::Icon::default().path("icons/trash.svg"))
                            .tooltip(gui::GUI_DESKTOP_REMOVE.text(locale))
                            .accessibility_label(gui::GUI_DESKTOP_REMOVE.text(locale))
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_environment_removal(id.clone(), locale, window, cx);
                            })),
                    )
                }
                _ => None,
            });

        table_line(
            locale,
            [
                label,
                connection,
                labels,
                status,
                settings_text(screen, Role::Small, version, ColorToken::TextMuted).truncate(),
                actions,
            ],
        )
        .min_h(px(60.))
        .py_2()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
    }
}

impl EditorTarget {
    fn key(&self) -> ConnectionKey {
        match self {
            Self::Builtin | Self::Add => ConnectionKey::Builtin,
            Self::Saved(entry, _) => ConnectionKey::Saved(entry.id.clone()),
            Self::Discovered(workspace) => ConnectionKey::Discovered {
                source: workspace.source.clone(),
                address: workspace.endpoint.address(),
            },
        }
    }
}

fn table_line(locale: Locale, cells: [Div; 6]) -> Div {
    row(locale).items_center().gap_3().px_3().children(
        cells
            .into_iter()
            .zip([
                Some(124.),
                None,
                Some(84.),
                Some(104.),
                Some(56.),
                Some(160.),
            ])
            .map(|(cell, width)| {
                div()
                    .min_w_0()
                    .when_some(width, |column, width| column.w(px(width)).flex_shrink_0())
                    .when(width.is_none(), |column| {
                        column.w_0().flex_1().overflow_hidden()
                    })
                    .child(cell)
            }),
    )
}

fn row_action(id: &'static str, index: usize, icon: IconName, label: &'static str) -> Button {
    Button::new((id, index))
        .ghost()
        .small()
        .icon(icon)
        .tooltip(label)
        .accessibility_label(label)
}

fn status_line(screen: &ScreenContext<'_>, label: marsa_ui::TextKey, token: ColorToken) -> Div {
    row(screen.locale)
        .items_center()
        .gap_1()
        .child(
            div()
                .size(px(6.))
                .flex_shrink_0()
                .rounded_full()
                .bg(color(screen.palette, token)),
        )
        .child(settings_text(screen, Role::Small, label.text(screen.locale), token).truncate())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_check_requires_a_running_and_healthy_guest() {
        assert!(!ConnectionCheck::Local(EngineState::Stopped, EngineHealthState::Ready).passed());
        assert!(!ConnectionCheck::Local(EngineState::Ready, EngineHealthState::Unhealthy).passed());
        assert!(ConnectionCheck::Local(EngineState::Idle, EngineHealthState::Ready).passed());
    }
}
