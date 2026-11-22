//! Run Compose on the project source explicitly chosen in the native file picker.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{collections::VecDeque, sync::Arc};

use marsa_client::compose::{ComposeCommand, ComposeOperation, ComposeOutput, ComposeSource};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, PathPromptOptions, Styled, div, px},
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{ScreenContext, panel},
    services::Services,
};

use super::{ResourceKind, ResourcesView, operations::Outcome, text, tone};

const MAX_OUTPUT_LINES: usize = 256;
const MAX_OUTPUT_BYTES: usize = 2048;

pub(super) enum Invocation {
    Standard(ComposeOperation),
    RemoveVolumes(String),
}

pub(super) struct ComposeOutputPane {
    project: String,
    lines: VecDeque<ComposeOutput>,
}

impl ResourcesView {
    pub(super) fn run_compose(
        &mut self,
        project: String,
        invocation: Invocation,
        service: Arc<Services>,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        let chosen_file = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(gui::GUI_DESKTOP_CHOOSE_COMPOSE_FILE.text(locale).into()),
        });
        self.busy = Some(ResourceKind::Compose);
        self.outcome = None;
        self.compose_output = None;
        self.operation = Some(cx.spawn(async move |view, cx| {
            let path = match chosen_file.await {
                Ok(Ok(paths)) => paths.and_then(|paths| paths.into_iter().next()),
                _ => {
                    finish(&view, cx, Some(Outcome::Rejected));
                    return;
                }
            };
            let Some(path) = path else {
                finish(&view, cx, None);
                return;
            };
            let command = match invocation {
                Invocation::Standard(operation) => {
                    ComposeCommand::for_selected_file(path, project.clone(), operation)
                }
                Invocation::RemoveVolumes(typed) => {
                    ComposeCommand::down_with_volumes_for_selected_file(
                        path,
                        project.clone(),
                        &typed,
                    )
                }
            };
            let Ok(command) = command else {
                finish(&view, cx, Some(Outcome::Rejected));
                return;
            };
            if view
                .update(cx, |view, cx| {
                    view.compose_output = Some(ComposeOutputPane {
                        project,
                        lines: VecDeque::new(),
                    });
                    cx.refresh_windows();
                })
                .is_err()
            {
                return;
            }
            let mut stream = service.compose(command);
            while let Some(batch) = stream.next().await {
                let Ok(lines) = batch else {
                    finish(&view, cx, Some(Outcome::Rejected));
                    return;
                };
                let result = view.update(cx, |view, cx| {
                    let Some(output) = &mut view.compose_output else {
                        return;
                    };
                    for mut line in lines {
                        if output.lines.len() == MAX_OUTPUT_LINES {
                            output.lines.pop_front();
                        }
                        if line.text.len() > MAX_OUTPUT_BYTES {
                            line.text
                                .truncate(line.text.floor_char_boundary(MAX_OUTPUT_BYTES));
                        }
                        output.lines.push_back(line);
                    }
                    cx.refresh_windows();
                });
                if result.is_err() {
                    return;
                }
            }
            let _ = service.refresh().result().await;
            finish(&view, cx, Some(Outcome::Success));
        }));
        cx.refresh_windows();
    }
}

fn finish(
    view: &marsa_ui::gpui::WeakEntity<ResourcesView>,
    cx: &mut marsa_ui::gpui::AsyncApp,
    outcome: Option<Outcome>,
) {
    let _ = view.update(cx, |view, cx| {
        view.busy = None;
        view.outcome = outcome.map(|outcome| (ResourceKind::Compose, outcome));
        cx.refresh_windows();
    });
}

pub(super) fn render_output(screen: &ScreenContext<'_>, view: &ResourcesView) -> Option<Div> {
    let output = view.compose_output.as_ref()?;
    Some(
        panel(screen)
            .child(text(
                screen,
                Role::Section,
                message(screen.locale, gui::GUI_DESKTOP_COMPOSE_OUTPUT),
            ))
            .child(text(screen, Role::Mono, output.project.clone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::S4.pixels()))
                    .p(px(Space::S12.pixels()))
                    .bg(tone(screen, ColorToken::BgCode))
                    .children(output.lines.iter().map(|line| {
                        let value = line.text.trim_end_matches(['\r', '\n']).to_owned();
                        let token = match line.source {
                            ComposeSource::Stdout => ColorToken::TextPrimary,
                            ComposeSource::Stderr => ColorToken::TextMuted,
                        };
                        text(screen, Role::Mono, value).text_color(tone(screen, token))
                    })),
            ),
    )
}
