//! Combine bounded, source-labelled logs for the selected Compose project.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::collections::VecDeque;

use marsa_client::docker::ContainerId;
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, Entity, ParentElement, Styled, Task, div, px},
    gpui_component::{Disableable, button::Button},
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{ScreenContext, panel},
    services::Services,
};

use super::{ResourcesView, logical_row, rows::ResourceRow, text, tone};

const MAX_LINES: usize = 256;
const MAX_LINE_BYTES: usize = 1024;

pub(super) struct ProjectLogs {
    project: String,
    lines: VecDeque<(String, String)>,
    failed: bool,
    _streams: Vec<Task<()>>,
}

impl ProjectLogs {
    fn follow(
        project: String,
        members: Vec<(ContainerId, String)>,
        service: &Services,
        cx: &Context<ResourcesView>,
    ) -> Self {
        let streams = members
            .into_iter()
            .map(|(id, source)| {
                let mut stream = service.logs(id);
                let project = project.clone();
                cx.spawn(async move |view, cx| {
                    while let Some(batch) = stream.next().await {
                        let failed = batch.is_err();
                        let updated = view.update(cx, |view, cx| {
                            let Some(logs) =
                                view.logs.as_mut().filter(|logs| logs.project == project)
                            else {
                                return;
                            };
                            match batch {
                                Ok(lines) => {
                                    for line in lines {
                                        if logs.lines.len() == MAX_LINES {
                                            logs.lines.pop_front();
                                        }
                                        let mut line = line.text;
                                        if line.len() > MAX_LINE_BYTES {
                                            line.truncate(line.floor_char_boundary(MAX_LINE_BYTES));
                                        }
                                        logs.lines.push_back((source.clone(), line));
                                    }
                                }
                                Err(_) => logs.failed = true,
                            }
                            cx.refresh_windows();
                        });
                        if updated.is_err() || failed {
                            break;
                        }
                    }
                })
            })
            .collect();
        Self {
            project,
            lines: VecDeque::new(),
            failed: false,
            _streams: streams,
        }
    }
}

pub(super) fn render(
    screen: &ScreenContext<'_>,
    row: &ResourceRow,
    view: &Entity<ResourcesView>,
    selected: Option<&ProjectLogs>,
) -> Div {
    let mut target = panel(screen).gap(px(Space::S8.pixels()));
    let selected = selected.filter(|logs| logs.project == row.id);
    let enabled = selected.is_none() && screen.services.is_some() && !row.members.is_empty();
    let project = row.id.clone();
    let members = row.members.clone();
    let service = screen.services.cloned();
    let view = view.clone();
    let action = if selected.is_some() {
        message(screen.locale, gui::GUI_DESKTOP_STOP_FOLLOWING)
    } else {
        message(screen.locale, gui::GUI_DESKTOP_FOLLOW_LOGS)
    };
    target = target.child(
        logical_row(screen.locale)
            .justify_between()
            .child(text(
                screen,
                Role::Section,
                message(screen.locale, gui::GUI_DESKTOP_COMBINED_LOGS),
            ))
            .child(
                Button::new("compose-follow-logs")
                    .outline()
                    .label(action)
                    .disabled(!enabled && selected.is_none())
                    .on_click(move |_, _, cx| {
                        view.update(cx, |view, cx| {
                            if view.logs.is_some() {
                                view.logs = None;
                            } else if let Some(service) = &service {
                                view.logs = Some(ProjectLogs::follow(
                                    project.clone(),
                                    members.clone(),
                                    service,
                                    cx,
                                ));
                            }
                            cx.refresh_windows();
                        });
                    }),
            ),
    );
    if let Some(logs) = selected {
        if logs.failed {
            target = target.child(
                text(
                    screen,
                    Role::Body,
                    message(
                        screen.locale,
                        gui::GUI_DESKTOP_A_CONTAINER_LOG_STREAM_ENDED_UNEXPECTEDLY,
                    ),
                )
                .text_color(tone(screen, ColorToken::StatusWarning)),
            );
        }
        if logs.lines.is_empty() {
            target = target.child(text(
                screen,
                Role::Small,
                message(screen.locale, gui::GUI_DESKTOP_WAITING_FOR_LOG_OUTPUT),
            ));
        } else {
            target = target.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::S4.pixels()))
                    .p(px(Space::S12.pixels()))
                    .bg(tone(screen, ColorToken::BgCode))
                    .children(logs.lines.iter().map(|(service, line)| {
                        logical_row(screen.locale)
                            .gap(px(Space::S8.pixels()))
                            .child(
                                text(screen, Role::Mono, service.clone())
                                    .text_color(tone(screen, ColorToken::SourceApi)),
                            )
                            .child(text(screen, Role::Mono, line.clone()).flex_1())
                    })),
            );
        }
    }
    target
}
