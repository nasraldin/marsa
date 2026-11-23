//! Combine bounded, source-labelled logs for the selected Compose project.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::collections::VecDeque;

use marsa_client::docker::{ContainerId, ExternalEndpoint, LogLine};
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, Entity, ParentElement, Styled, Task, div, px},
    gpui_component::{Disableable, button::Button},
    preferences::LogSettings,
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{ScreenContext, panel},
    services::Services,
};

use super::{ResourcesView, logical_row, rows::ResourceRow, text, tone};

const MAX_LINE_BYTES: usize = 1024;

pub(super) struct ProjectLogs {
    project: String,
    endpoint: Option<ExternalEndpoint>,
    lines: VecDeque<(String, LogLine)>,
    failed: bool,
    _streams: Vec<Task<()>>,
}

impl ProjectLogs {
    fn append(&mut self, source: &str, lines: Vec<LogLine>, limit: usize) {
        for mut line in lines {
            if self.lines.len() == limit {
                self.lines.pop_front();
            }
            let timestamp_bytes = line.retained_bytes() - line.text.len();
            let text_limit = MAX_LINE_BYTES - timestamp_bytes;
            if line.text.len() > text_limit {
                let end = line.text.floor_char_boundary(text_limit);
                line.text = line.text[..end].to_owned();
            }
            self.lines.push_back((source.to_owned(), line));
        }
    }

    pub(super) fn trim(&mut self, limit: usize) {
        self.lines.drain(..self.lines.len().saturating_sub(limit));
    }

    fn follow(
        project: String,
        members: Vec<(ContainerId, String)>,
        service: &Services,
        endpoint: Option<ExternalEndpoint>,
        cx: &Context<ResourcesView>,
    ) -> Self {
        let streams = members
            .into_iter()
            .take(32)
            .map(|(id, source)| {
                let mut stream = match &endpoint {
                    Some(endpoint) => service.external_logs(endpoint.clone(), id),
                    None => service.logs(id),
                };
                let project = project.clone();
                cx.spawn(async move |view, cx| {
                    while let Some(batch) = stream.next().await {
                        let failed = batch.is_err();
                        let updated = view.update(cx, |view, cx| {
                            let limit = view.log_settings.buffer.stack_lines();
                            let Some(logs) =
                                view.logs.as_mut().filter(|logs| logs.project == project)
                            else {
                                return;
                            };
                            match batch {
                                Ok(lines) => {
                                    logs.append(&source, lines, limit);
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
            endpoint,
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
    settings: LogSettings,
) -> Div {
    let mut target = panel(screen).gap(px(Space::S8.pixels()));
    let selected =
        selected.filter(|logs| logs.project == row.id && logs.endpoint.as_ref() == row.endpoint());
    let endpoint = row.endpoint().cloned();
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
                            if view.logs.as_ref().is_some_and(|logs| {
                                logs.project == project && logs.endpoint == endpoint
                            }) {
                                view.logs = None;
                            } else if let Some(service) = &service {
                                view.logs = Some(ProjectLogs::follow(
                                    project.clone(),
                                    members.clone(),
                                    service,
                                    endpoint.clone(),
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
                            .children(line.timestamp.map(|timestamp| {
                                text(
                                    screen,
                                    Role::Mono,
                                    timestamp.format(settings.local_timestamps),
                                )
                                .text_color(tone(screen, ColorToken::TextMuted))
                            }))
                            .child(text(screen, Role::Mono, line.text.clone()).flex_1())
                    })),
            );
        }
    }
    target
}

#[cfg(test)]
mod tests {
    use marsa_client::docker::LogSource;
    use marsa_ui::preferences::LogBufferSize;

    use super::*;

    #[test]
    fn stack_buffer_keeps_recent_sources_and_truncates_large_lines_at_character_boundaries() {
        let mut logs = ProjectLogs {
            project: "test-project".into(),
            endpoint: None,
            lines: VecDeque::new(),
            failed: false,
            _streams: Vec::new(),
        };
        let limit = LogBufferSize::Compact.stack_lines();
        logs.append(
            "service-a",
            (0..256)
                .map(|index| LogLine {
                    source: LogSource::Stdout,
                    text: format!("{index} {}", "ي".repeat(1024)),
                    timestamp: None,
                })
                .collect(),
            limit,
        );
        assert_eq!(logs.lines.len(), 64);
        let (source, line) = logs.lines.front().unwrap();
        assert_eq!(source, "service-a");
        assert!(line.text.starts_with("192 "));
        assert_eq!(line.text.len(), 1024);
    }
}
