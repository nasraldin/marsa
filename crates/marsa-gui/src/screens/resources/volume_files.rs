//! The Volumes route offers reads only through the owner-bound guest provider.

use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

use marsa_client::docker::{
    ContainerFileFormat, ContainerPath, SelectedVolumeDownload, VolumeDownloadError, VolumeSummary,
};
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        App, AppContext as _, Context, Div, Entity, ParentElement as _, Styled as _, Task, Window,
        div, px,
    },
    gpui_component::{
        Disableable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
    },
    gui_keys as gui,
    preferences::DownloadFormat,
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{
        ScreenContext, align, color, dialog_actions, dialog_frame, dialog_heading, resource_input,
        row, technical,
    },
    services::Services,
};

use super::{ResourceKind, ResourceRow, ResourcesView};

#[derive(Default)]
pub(super) struct DownloadState {
    task: Option<Task<()>>,
    result: Option<(VolumeIdentity, Result<(), VolumeDownloadError>)>,
}

#[derive(Clone, PartialEq, Eq)]
struct VolumeIdentity {
    name: String,
    created_at: Option<String>,
}

impl From<&VolumeSummary> for VolumeIdentity {
    fn from(volume: &VolumeSummary) -> Self {
        Self {
            name: volume.name.clone(),
            created_at: volume.created_at.clone(),
        }
    }
}

pub(super) fn render(
    screen: &ScreenContext<'_>,
    selected: &ResourceRow,
    entity: &Entity<ResourcesView>,
    view: &ResourcesView,
) -> Div {
    let locale = screen.locale;
    let palette = screen.palette;
    let mut pane = div().flex().flex_col().gap(px(Space::S8.pixels()));
    let volumes = screen.state.volumes();
    let volume = selected
        .source()
        .is_none()
        .then(|| volumes.get(&selected.id))
        .flatten()
        .filter(|volume| {
            volume.driver == "local" && volume.scope == "local" && volume.created_at.is_some()
        });
    let Some(volume) = volume else {
        return pane.child(super::text(
            screen,
            Role::Small,
            gui::GUI_VOLUME_DOWNLOAD_UNSUPPORTED.text(locale),
        ));
    };
    let service = screen.services.cloned();
    let selected_volume = volume.clone();
    let entity = entity.clone();
    pane = pane.child(
        row(locale).child(
            Button::new("download-volume-files")
                .outline()
                .label(gui::GUI_VOLUME_DOWNLOAD.text(locale))
                .disabled(service.is_none() || view.volume_download.task.is_some())
                .on_click(move |_, window, cx| {
                    if let Some(service) = &service {
                        entity.update(cx, |view, cx| {
                            view.open_volume_download(
                                selected_volume.clone(),
                                Arc::clone(service),
                                (locale, palette),
                                window,
                                cx,
                            );
                        });
                    }
                }),
        ),
    );
    if view.volume_download.task.is_some() {
        pane = pane.child(gui::GUI_DESKTOP_WORKING.text(locale));
    }
    if let Some((identity, result)) = &view.volume_download.result
        && *identity == VolumeIdentity::from(volume)
    {
        let (key, tone) = match result {
            Ok(()) => (gui::GUI_CONTAINER_DOWNLOAD_SAVED, ColorToken::StatusRunning),
            Err(error) => (error_label(error), ColorToken::StatusWarning),
        };
        pane = pane.child(
            super::text(screen, Role::Small, key.text(locale)).text_color(color(palette, tone)),
        );
    }
    pane
}

impl ResourcesView {
    fn open_volume_download(
        &mut self,
        volume: VolumeSummary,
        services: Arc<Services>,
        appearance: (Locale, Palette),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.volume_download.task.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let identity = VolumeIdentity::from(&volume);
        let mut request = services
            .request(move |client| async move { Ok(client.select_volume_download(&volume).await) });
        self.volume_download.result = None;
        self.volume_download.task = Some(cx.spawn_in(window, async move |owner, cx| {
            let result = request
                .result()
                .await
                .and_then(Result::ok)
                .unwrap_or(Err(VolumeDownloadError::Transfer));
            let _ = cx.update(|window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.volume_download.task = None;
                    let still_selected = view.selected_endpoint.is_none()
                        && view.selection.as_ref().is_some_and(|(kind, key)| {
                            *kind == ResourceKind::Volumes && *key == identity.name
                        });
                    if still_selected && !window.has_active_dialog(cx) {
                        match result {
                            Ok(pinned) => view.review_volume_path(
                                identity, pinned, services, appearance, window, cx,
                            ),
                            Err(error) => {
                                view.volume_download.result = Some((identity, Err(error)))
                            }
                        }
                    }
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }

    fn review_volume_path(
        &self,
        identity: VolumeIdentity,
        pinned: SelectedVolumeDownload,
        services: Arc<Services>,
        (locale, palette): (Locale, Palette),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefer_raw = self.log_settings.download_format == DownloadFormat::SingleFile;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("/"));
        cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.refresh_windows())
            .detach();
        let owner = cx.weak_entity();
        let pinned = Rc::new(RefCell::new(Some(pinned)));
        window.open_dialog(cx, move |dialog, _, cx| {
            let path = input.read(cx).value().to_string();
            let valid = ContainerPath::parse(path.clone()).is_ok();
            let identity = identity.clone();
            let selected = technical(&identity.name);
            let pinned = Rc::clone(&pinned);
            let owner = owner.clone();
            let services = Arc::clone(&services);
            let submit = Rc::new(move |cx: &mut App| {
                if !valid {
                    return false;
                }
                let Some(pinned) = pinned.borrow_mut().take() else {
                    return false;
                };
                owner
                    .update(cx, |view, cx| {
                        view.download_volume(
                            identity.clone(),
                            pinned,
                            path.clone(),
                            prefer_raw,
                            Arc::clone(&services),
                            cx,
                        );
                    })
                    .is_ok()
            });
            let enter = Rc::clone(&submit);
            dialog_frame(dialog, 512.)
                .title(dialog_heading(
                    locale,
                    palette,
                    gui::GUI_VOLUME_DOWNLOAD.text(locale),
                ))
                .child(
                    align(div().flex().flex_col().gap(px(Space::S8.pixels())), locale)
                        .child(crate::screens::external_inventory::dialog_scope(
                            locale,
                            palette,
                            "Marsa".into(),
                        ))
                        .child(selected)
                        .child(gui::GUI_VOLUME_DOWNLOAD_PATH.text(locale))
                        .child(
                            resource_input(&input)
                                .aria_label(gui::GUI_VOLUME_DOWNLOAD_PATH.text(locale)),
                        ),
                )
                .on_ok(move |_, _, cx| enter(cx))
                .footer(
                    dialog_actions(locale, palette)
                        .child(
                            Button::new("cancel-volume-download")
                                .outline()
                                .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-volume-download")
                                .primary()
                                .label(gui::GUI_CONTAINER_DOWNLOAD_CONTINUE.text(locale))
                                .disabled(!valid)
                                .on_click(move |_, window, cx| {
                                    if submit(cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn download_volume(
        &mut self,
        identity: VolumeIdentity,
        pinned: SelectedVolumeDownload,
        path: String,
        prefer_raw: bool,
        services: Arc<Services>,
        cx: &mut Context<Self>,
    ) {
        if self.volume_download.task.is_some() {
            return;
        }
        self.volume_download.result = None;
        let filename = path
            .rsplit('/')
            .find(|name| !name.is_empty())
            .unwrap_or(&identity.name)
            .to_owned();
        let mut request =
            services.pending(async move { Ok(pinned.prepare(path, prefer_raw).await) });
        self.volume_download.task = Some(cx.spawn(async move |view, cx| {
            let result = match request.result().await {
                Some(Ok(Ok(download))) => {
                    let filename = match download.format() {
                        ContainerFileFormat::Tar => format!("{filename}.tar"),
                        ContainerFileFormat::SingleFile => filename,
                    };
                    let picker =
                        cx.update(|cx| cx.prompt_for_new_path(Path::new("."), Some(&filename)));
                    if let Ok(Ok(Some(destination))) = picker.await {
                        let mut transfer =
                            services.pending(async move { Ok(download.save(&destination).await) });
                        Some(
                            transfer
                                .result()
                                .await
                                .and_then(Result::ok)
                                .unwrap_or(Err(VolumeDownloadError::Transfer)),
                        )
                    } else {
                        None
                    }
                }
                Some(Ok(Err(error))) => Some(Err(error)),
                _ => Some(Err(VolumeDownloadError::Transfer)),
            };
            let _ = view.update(cx, |view, cx| {
                view.volume_download.task = None;
                view.volume_download.result = result.map(|result| (identity, result));
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn error_label(error: &VolumeDownloadError) -> TextKey {
    match error {
        VolumeDownloadError::Unsupported => gui::GUI_VOLUME_DOWNLOAD_UNSUPPORTED,
        VolumeDownloadError::Unavailable => gui::GUI_VOLUME_DOWNLOAD_UNAVAILABLE,
        VolumeDownloadError::Changed => gui::GUI_VOLUME_DOWNLOAD_CHANGED,
        VolumeDownloadError::InvalidPath => gui::GUI_VOLUME_DOWNLOAD_INVALID_PATH,
        VolumeDownloadError::Limit => gui::GUI_VOLUME_DOWNLOAD_LIMIT,
        VolumeDownloadError::Transfer => gui::GUI_VOLUME_DOWNLOAD_FAILED,
        VolumeDownloadError::Destination(error)
            if error.kind() == std::io::ErrorKind::CrossesDevices =>
        {
            gui::GUI_VOLUME_DOWNLOAD_SAME_FILESYSTEM
        }
        VolumeDownloadError::Destination(_) => gui::GUI_VOLUME_DOWNLOAD_DESTINATION,
    }
}
