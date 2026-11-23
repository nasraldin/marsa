//! An explicit path and destination for downloading from the pinned container.

use std::path::Path;

use marsa_client::docker::{ContainerFileFormat, ContainerPath};
use marsa_ui::{
    ColorToken,
    gpui::{AppContext as _, Context, Div, ParentElement as _, Styled as _, Window, div, px},
    gpui_component::{
        Disableable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
    },
    gui_keys as gui,
    preferences::DownloadFormat,
};

use crate::screens::common::{ScreenContext, color, panel, resource_input, row, technical};

use super::{ContainerTarget, ContainerView};

pub(super) fn render(
    view: &ContainerView,
    screen: &ScreenContext<'_>,
    cx: &Context<ContainerView>,
) -> Div {
    let locale = screen.locale;
    let mut pane = panel(screen)
        .child(
            screen
                .typography(marsa_ui::tokens::typography::Role::Body)
                .apply(div())
                .max_w(px(480.))
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .child(gui::GUI_CONTAINER_DOWNLOAD_HELP.text(locale)),
        )
        .child(
            row(locale).child(
                Button::new("container-download")
                    .primary()
                    .label(gui::GUI_CONTAINER_DOWNLOAD_PATH.text(locale))
                    .disabled(view.file_operation.is_some() || view.services.is_none())
                    .on_click(
                        cx.listener(|view, _, window, cx| view.open_file_download(window, cx)),
                    ),
            ),
        );
    if view.file_operation.is_some() {
        pane = pane.child(gui::GUI_DESKTOP_WORKING.text(locale));
    }
    if let Some((target, success)) = &view.file_result
        && view.focused.as_ref() == Some(target)
    {
        let (label, token) = if *success {
            (gui::GUI_CONTAINER_DOWNLOAD_SAVED, ColorToken::StatusRunning)
        } else {
            (
                gui::GUI_CONTAINER_DOWNLOAD_FAILED,
                ColorToken::StatusWarning,
            )
        };
        pane = pane.child(
            div()
                .text_color(color(screen.palette, token))
                .child(label.text(locale)),
        );
    }
    pane
}

impl ContainerView {
    fn open_file_download(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.focused.clone() else {
            return;
        };
        if self.file_operation.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let prefer_raw = self.logs.settings().download_format == DownloadFormat::SingleFile;
        let locale = self.locale;
        let source = target
            .endpoint()
            .and_then(|endpoint| self.workspace(endpoint))
            .map_or_else(|| "Marsa".into(), |engine| technical(&engine.source));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("/etc/os-release"));
        cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.refresh_windows())
            .detach();
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let raw_path = input.read(cx).value();
            let validated = ContainerPath::parse(raw_path.as_ref()).ok();
            let name = raw_path
                .rsplit('/')
                .find(|name| !name.is_empty())
                .unwrap_or("container-files")
                .to_owned();
            let owner = owner.clone();
            let target = target.clone();
            let label = gui::GUI_CONTAINER_DOWNLOAD_PATH.text(locale);
            dialog.title(label).child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(source.clone())
                    .child(gui::GUI_CONTAINER_DOWNLOAD_ABSOLUTE.text(locale))
                    .child(resource_input(&input).aria_label(label))
                    .child(
                        row(locale)
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("cancel-file-download")
                                    .outline()
                                    .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("confirm-file-download")
                                    .primary()
                                    .label(gui::GUI_CONTAINER_DOWNLOAD_CONTINUE.text(locale))
                                    .disabled(validated.is_none())
                                    .on_click(move |_, window, cx| {
                                        if let Some(path) = validated.clone() {
                                            let _ = owner.update(cx, |view, cx| {
                                                view.download_file(
                                                    target.clone(),
                                                    path,
                                                    name.clone(),
                                                    prefer_raw,
                                                    cx,
                                                );
                                            });
                                            window.close_dialog(cx);
                                        }
                                    }),
                            ),
                    ),
            )
        });
    }

    fn download_file(
        &mut self,
        target: ContainerTarget,
        path: ContainerPath,
        name: String,
        prefer_raw: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(services) = self.services.clone() else {
            return;
        };
        if self.file_operation.is_some() {
            return;
        }
        self.file_result = None;
        let docker = services.resource_docker(target.endpoint().cloned());
        let id = target.id().clone();
        let request_path = path.clone();
        let requester = docker.clone();
        let mut format = services.pending(async move {
            Ok(requester
                .container_file_format(&id, &request_path, prefer_raw)
                .await?)
        });
        self.file_operation = Some(cx.spawn(async move |view, cx| {
            let result = match format.result().await {
                Some(Ok(format)) => {
                    let suggested = if format == ContainerFileFormat::Tar {
                        format!("{name}.tar")
                    } else {
                        name
                    };
                    let dialog =
                        cx.update(|cx| cx.prompt_for_new_path(Path::new("."), Some(&suggested)));
                    if let Ok(Ok(Some(destination))) = dialog.await {
                        let id = target.id().clone();
                        let mut request = services.pending(async move {
                            Ok(docker
                                .download_container_file(&id, &path, format, &destination)
                                .await?)
                        });
                        Some(matches!(request.result().await, Some(Ok(()))))
                    } else {
                        None
                    }
                }
                _ => Some(false),
            };
            let _ = view.update(cx, |view, cx| {
                view.file_operation = None;
                view.file_result = result.map(|success| (target, success));
                cx.notify();
            });
        }));
        cx.notify();
    }
}
