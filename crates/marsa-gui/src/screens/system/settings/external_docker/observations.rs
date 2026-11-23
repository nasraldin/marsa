//! Read current events and inventory from the environment the editor was opened for.

use marsa_client::docker::{DockerActivity, EnvironmentReport};
use marsa_ui::{
    ColorToken, TextKey,
    gpui::{ClipboardItem, Context, Div, ParentElement as _, Styled as _, Window, div},
    gpui_component::{Disableable as _, button::Button},
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{
    SystemView,
    editor::{Editor, EditorTarget},
    settings_text,
};
use crate::screens::{
    common::{ScreenContext, color, row, technical},
    system::activity::utc_time,
};

#[derive(Default)]
pub(super) struct Observations {
    activity: Load<Vec<DockerActivity>>,
    report: Load<EnvironmentReport>,
}

#[derive(Default)]
enum Load<T> {
    #[default]
    Empty,
    Loading,
    Ready(T),
    Failed,
}

enum ResultPage {
    Activity(Vec<DockerActivity>),
    Report(EnvironmentReport),
}

impl SystemView {
    pub(super) fn load_environment_observations(
        &mut self,
        report: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.environment_settings.editor else {
            return;
        };
        let Some(services) = &self.services else {
            return;
        };
        let Some(target) = editor
            .workspace(cx)
            .map(|workspace| Some(workspace.endpoint.clone()))
            .or_else(|| matches!(editor.target, EditorTarget::Builtin).then_some(None))
        else {
            return;
        };
        let docker = services.resource_docker(target);
        if report {
            if matches!(editor.observations.report, Load::Loading) {
                return;
            }
            editor.observations.report = Load::Loading;
        } else {
            if matches!(editor.observations.activity, Load::Loading) {
                return;
            }
            editor.observations.activity = Load::Loading;
        }
        let token = editor.observations_token.clone();
        let mut pending = services.pending(async move {
            if report {
                let containers = docker.containers().await?;
                let images = docker.images().await?;
                let volumes = docker.volumes().await.ok().map(|items| items.len());
                let networks = docker.networks().await.ok().map(|items| items.len());
                Ok(ResultPage::Report(EnvironmentReport::from_inventory(
                    &containers,
                    images.len(),
                    volumes,
                    networks,
                )))
            } else {
                Ok(ResultPage::Activity(docker.recent_activity().await?))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = pending.result().await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(editor) = &mut this.environment_settings.editor else {
                        return;
                    };
                    if !std::rc::Rc::ptr_eq(&editor.observations_token, &token) {
                        return;
                    }
                    match result {
                        Some(Ok(ResultPage::Activity(events))) => {
                            editor.observations.activity = Load::Ready(events)
                        }
                        Some(Ok(ResultPage::Report(snapshot))) => {
                            editor.observations.report = Load::Ready(snapshot)
                        }
                        _ if report => editor.observations.report = Load::Failed,
                        _ => editor.observations.activity = Load::Failed,
                    }
                    cx.notify();
                });
                window.refresh();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn environment_activity(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let page = &editor.observations.activity;
        let mut body = self.observation_header(
            screen,
            gui::GUI_ENVIRONMENT_ACTIVITY_PERIOD,
            false,
            matches!(page, Load::Loading),
            cx,
        );
        if let Load::Ready(events) = page {
            body = body.children(events.iter().map(|event| {
                detail(
                    screen,
                    &event.subject,
                    &format!(
                        "{} / {} · {}",
                        event.kind,
                        event.action,
                        utc_time(event.timestamp.saturating_mul(1000))
                    ),
                )
            }));
        }
        body.children(status(
            screen,
            page,
            matches!(page, Load::Ready(events) if events.is_empty()),
            gui::GUI_ENVIRONMENT_NO_EVENTS,
        ))
    }

    pub(super) fn environment_report(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let page = &editor.observations.report;
        let locale = screen.locale;
        let mut body = self.observation_header(
            screen,
            gui::GUI_ENVIRONMENT_REPORT_HELP,
            true,
            matches!(page, Load::Loading),
            cx,
        );
        if let Load::Ready(snapshot) = page {
            let mut lines = Vec::new();
            for (label, count) in [
                (gui::GUI_DESKTOP_CONTAINERS, Some(snapshot.containers.len())),
                (gui::GUI_DESKTOP_IMAGES, Some(snapshot.image_count)),
                (gui::GUI_DESKTOP_VOLUMES, snapshot.volume_count),
                (gui::GUI_DESKTOP_NETWORKS, snapshot.network_count),
            ] {
                let value = count.map_or_else(
                    || gui::GUI_DESKTOP_UNAVAILABLE.text(locale).to_owned(),
                    |count| count.to_string(),
                );
                lines.push(format!("{}  {}", label.text(locale), value));
            }
            let mut clipboard = lines.join("\n");
            for container in &snapshot.containers {
                use std::fmt::Write as _;
                let _ = write!(
                    clipboard,
                    "\n{}\t{}\t{}",
                    container.name, container.image, container.state
                );
            }
            body = body
                .child(row(locale).gap_2().flex_wrap().children(
                    lines.into_iter().map(|text| {
                        settings_text(screen, Role::Label, text, ColorToken::TextPrimary)
                    }),
                ))
                .child(
                    row(locale).child(
                        Button::new("copy-environment-report")
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_REPORT_COPY.text(locale))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(clipboard.clone()))
                            }),
                    ),
                )
                .children(snapshot.containers.iter().map(|container| {
                    detail(
                        screen,
                        &container.name,
                        &format!("{} · {}", container.state, container.image),
                    )
                }));
        }
        body.children(status(screen, page, false, gui::GUI_ENVIRONMENT_NO_EVENTS))
    }

    fn observation_header(
        &self,
        screen: &ScreenContext<'_>,
        help: TextKey,
        report: bool,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        div().flex().flex_col().gap_3().child(
            row(screen.locale)
                .gap_3()
                .items_center()
                .child(
                    settings_text(
                        screen,
                        Role::Small,
                        help.text(screen.locale),
                        ColorToken::TextMuted,
                    )
                    .flex_1(),
                )
                .child(
                    Button::new("refresh-environment-observations")
                        .outline()
                        .label(gui::GUI_DESKTOP_REFRESH.text(screen.locale))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.load_environment_observations(report, window, cx)
                        })),
                ),
        )
    }
}

fn detail(screen: &ScreenContext<'_>, title: &str, metadata: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .py_2()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(settings_text(
            screen,
            Role::Label,
            technical(title),
            ColorToken::TextPrimary,
        ))
        .child(settings_text(
            screen,
            Role::Small,
            technical(metadata),
            ColorToken::TextMuted,
        ))
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
        Load::Failed => (gui::GUI_ENVIRONMENT_OFFLINE, ColorToken::StatusWarning),
    };
    Some(settings_text(
        screen,
        Role::Small,
        label.text(screen.locale),
        token,
    ))
}
