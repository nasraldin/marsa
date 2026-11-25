//! Dispatch resource commands and load selected image metadata outside the UI thread.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::sync::Arc;

use marsa_client::{
    compose::ComposeOperation,
    docker::{
        ContainerAction, ContainerId, ExternalEndpoint, ImageId, ImageReference, ImageRemoval,
        ImageRun, NetworkId, NetworkRemoval, RemovalMode, ResourceName, VolumeRemoval,
    },
};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{AnyElement, Context, Div, ParentElement, PathPromptOptions, Styled, div, px},
    tokens::{scale::Space, typography::Role},
};

use crate::{
    desktop::toast::{self, Tone},
    screens::common::ScreenContext,
    services::{ServiceError, Services},
};

use super::{
    ResourceKind, ResourceRequest, ResourcesView, compose::Invocation, logical_row,
    rows::ResourceRow, text, tone,
};

/// What succeeded — drives the toast copy instead of a generic "completed".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SuccessKind {
    ImagePulled,
    ImageRemoved,
    ImagesRemoved,
    ImageImported,
    ImageExported,
    ImagePushed,
    ImageTagged,
    ContainerStarted,
    ContainerCreated,
    VolumeCreated,
    VolumeRemoved,
    NetworkCreated,
    NetworkRemoved,
    ContainerAttached,
    ContainerDetached,
    ComposeUp,
    ComposeRestart,
    ComposeDown,
    ComposeCreated,
}

impl SuccessKind {
    fn message(self, locale: Locale) -> marsa_ui::gpui::SharedString {
        message(
            locale,
            match self {
                Self::ImagePulled => gui::GUI_DESKTOP_TOAST_IMAGE_PULLED,
                Self::ImageRemoved => gui::GUI_DESKTOP_TOAST_IMAGE_REMOVED,
                Self::ImagesRemoved => gui::GUI_DESKTOP_TOAST_IMAGES_REMOVED,
                Self::ImageImported => gui::GUI_DESKTOP_TOAST_IMAGE_IMPORTED,
                Self::ImageExported => gui::GUI_DESKTOP_TOAST_IMAGE_EXPORTED,
                Self::ImagePushed => gui::GUI_DESKTOP_TOAST_IMAGE_PUSHED,
                Self::ImageTagged => gui::GUI_DESKTOP_TOAST_IMAGE_TAGGED,
                Self::ContainerStarted => gui::GUI_DESKTOP_TOAST_CONTAINER_STARTED,
                Self::ContainerCreated => gui::GUI_DESKTOP_TOAST_CONTAINER_CREATED,
                Self::VolumeCreated => gui::GUI_DESKTOP_TOAST_VOLUME_CREATED,
                Self::VolumeRemoved => gui::GUI_DESKTOP_TOAST_VOLUME_REMOVED,
                Self::NetworkCreated => gui::GUI_DESKTOP_TOAST_NETWORK_CREATED,
                Self::NetworkRemoved => gui::GUI_DESKTOP_TOAST_NETWORK_REMOVED,
                Self::ContainerAttached => gui::GUI_DESKTOP_TOAST_CONTAINER_ATTACHED,
                Self::ContainerDetached => gui::GUI_DESKTOP_TOAST_CONTAINER_DETACHED,
                Self::ComposeUp => gui::GUI_DESKTOP_TOAST_COMPOSE_UP,
                Self::ComposeRestart => gui::GUI_DESKTOP_TOAST_COMPOSE_RESTART,
                Self::ComposeDown => gui::GUI_DESKTOP_TOAST_COMPOSE_DOWN,
                Self::ComposeCreated => gui::GUI_DESKTOP_TOAST_COMPOSE_CREATED,
            },
        )
    }
}

#[derive(Clone, Copy)]
pub(super) enum Outcome {
    Success(SuccessKind),
    Rejected,
    Unavailable,
    ImageCreated,
    ComposeFileExists,
    ComposeFileWriteFailed,
    ComposeFileKept,
    ComposeKeychainFailed,
    UnsupportedImagePlatform,
    Cancelled,
}

impl Outcome {
    pub(super) fn is_success(self) -> bool {
        matches!(self, Self::Success(_))
    }

    fn toast_key(self) -> usize {
        match self {
            Self::Success(kind) => 100 + kind as usize,
            Self::Rejected => 1,
            Self::Unavailable => 2,
            Self::ImageCreated => 3,
            Self::ComposeFileExists => 4,
            Self::ComposeFileWriteFailed => 5,
            Self::ComposeFileKept => 6,
            Self::ComposeKeychainFailed => 9,
            Self::UnsupportedImagePlatform => 7,
            Self::Cancelled => 8,
        }
    }
}

impl ResourcesView {
    pub(super) fn measure_volumes(
        &mut self,
        service: &Services,
        endpoint: Option<ExternalEndpoint>,
        cx: &mut Context<Self>,
    ) {
        if self.volume_scanning {
            return;
        }
        self.volume_scanning = true;
        self.volume_scan_failed = false;
        let mut request = service.volume_usage(endpoint.clone());
        self.volume_scan = Some(cx.spawn(async move |view, cx| {
            let result = request.result().await;
            let _ = view.update(cx, |view, cx| {
                view.volume_scanning = false;
                match result {
                    Some(Ok(usage)) => {
                        if let Some(endpoint) = endpoint {
                            view.external_volume_usage
                                .retain(|(current, _)| current != &endpoint);
                            view.external_volume_usage.push((endpoint, usage));
                        } else {
                            view.volume_usage = usage;
                        }
                    }
                    _ => view.volume_scan_failed = true,
                }
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }

    pub(super) fn select(
        &mut self,
        kind: ResourceKind,
        key: String,
        resource: (&str, Option<ExternalEndpoint>),
        services: Option<&Services>,
        cx: &mut Context<Self>,
    ) {
        let (id, endpoint) = resource;
        self.selection = Some((kind, key.clone()));
        self.selected_endpoint = endpoint.clone();
        self.form = None;
        self.logs = None;
        self.inspection_task = None;
        self.inspect = None;
        self.inspection_failed = false;
        if kind == ResourceKind::Images {
            if let (Some(services), Ok(id)) = (services, ImageId::parse(id)) {
                let mut request = services.inspect_image(id, endpoint);
                self.inspection_task = Some(cx.spawn(async move |view, cx| {
                    let result = request.result().await;
                    let _ =
                        view.update(cx, |view, cx| {
                            if view.selection.as_ref().is_some_and(|(selected, current)| {
                                *selected == kind && current == &key
                            }) {
                                view.inspection_task = None;
                                match result {
                                    Some(Ok(details)) => view.inspect = Some(details),
                                    _ => view.inspection_failed = true,
                                }
                                cx.refresh_windows();
                            }
                        });
                }));
            } else {
                self.inspection_failed = true;
            }
        }
        cx.refresh_windows();
    }

    pub(super) fn external_project_action(
        &mut self,
        service: &Services,
        endpoint: ExternalEndpoint,
        members: Vec<ContainerId>,
        action: ContainerAction,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let success = match action {
            ContainerAction::Start | ContainerAction::Unpause => SuccessKind::ComposeUp,
            ContainerAction::Stop | ContainerAction::Pause => SuccessKind::ComposeDown,
            ContainerAction::Restart => SuccessKind::ComposeRestart,
        };
        let mut operation = service.pending(async move {
            let mut failed = false;
            for id in members {
                failed |= endpoint.act(&id, action).await.is_err();
            }
            Ok(!failed)
        });
        self.busy = Some(ResourceKind::Compose);
        self.outcome = None;
        self.operation = Some(cx.spawn(async move |view, cx| {
            let outcome = if matches!(operation.result().await, Some(Ok(true))) {
                Outcome::Success(success)
            } else {
                Outcome::Rejected
            };
            let _ = view.update(cx, |view, cx| {
                view.busy = None;
                view.outcome = Some((ResourceKind::Compose, outcome));
                cx.emit(super::ExternalResourcesChanged);
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }

    pub(super) fn dispatch(
        &mut self,
        kind: ResourceKind,
        request: (ResourceRequest, Option<ExternalEndpoint>),
        service: Arc<Services>,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let (request, endpoint) = request;
        let request = match request {
            ResourceRequest::CreateStack { project, yaml } => {
                self.run_compose(
                    project,
                    (Invocation::Create(yaml), endpoint),
                    service,
                    locale,
                    cx,
                );
                return;
            }
            ResourceRequest::DeployStack { project, keychain } => {
                let invocation = if keychain {
                    Invocation::Keychain
                } else {
                    Invocation::Standard(ComposeOperation::Up)
                };
                self.run_compose(project, (invocation, endpoint), service, locale, cx);
                return;
            }
            ResourceRequest::ComposeUp(project) => {
                self.run_compose(
                    project,
                    (Invocation::Standard(ComposeOperation::Up), endpoint),
                    service,
                    locale,
                    cx,
                );
                return;
            }
            ResourceRequest::ComposeRestart(project) => {
                self.run_compose(
                    project,
                    (Invocation::Standard(ComposeOperation::Restart), endpoint),
                    service,
                    locale,
                    cx,
                );
                return;
            }
            ResourceRequest::ComposeDown {
                project,
                volumes,
                typed,
            } => {
                let invocation = if volumes {
                    Invocation::RemoveVolumes(typed)
                } else {
                    Invocation::Standard(ComposeOperation::Down)
                };
                self.run_compose(project, (invocation, endpoint), service, locale, cx);
                return;
            }
            request => request,
        };
        let import = matches!(request, ResourceRequest::ImportImage).then(|| {
            cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(gui::GUI_DESKTOP_IMPORT_IMAGE.text(locale).into()),
            })
        });
        let export = matches!(request, ResourceRequest::ExportImage(_))
            .then(|| cx.prompt_for_new_path(std::path::Path::new("."), Some("image.tar")));
        self.pull = None;
        self.busy = Some(kind);
        self.outcome = None;
        self.operation = Some(cx.spawn(async move |view, cx| {
            let external = endpoint.is_some();
            let file = match (import, export) {
                (Some(prompt), _) => match prompt.await {
                    Ok(Ok(files)) => files.and_then(|files| files.into_iter().next()),
                    _ => None,
                },
                (_, Some(prompt)) => match prompt.await {
                    Ok(Ok(file)) => file,
                    _ => None,
                },
                _ => None,
            };
            let result = if matches!(
                request,
                ResourceRequest::ImportImage | ResourceRequest::ExportImage(_)
            ) && file.is_none()
            {
                Ok(Some(Outcome::Cancelled))
            } else {
                dispatch(&service, endpoint, request, file).await
            };
            let outcome = match result {
                Ok(Some(outcome)) => {
                    if !external && !matches!(outcome, Outcome::Cancelled) {
                        let _ = service.refresh().result().await;
                    }
                    outcome
                }
                Ok(None) => Outcome::Unavailable,
                Err(_) => Outcome::Rejected,
            };
            let _ = view.update(cx, |view, cx| {
                view.busy = None;
                if kind == ResourceKind::Volumes && outcome.is_success() {
                    view.volume_usage.clear();
                    view.external_volume_usage.clear();
                }
                view.outcome = Some((kind, outcome));
                if external {
                    cx.emit(super::ExternalResourcesChanged);
                }
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }
}

async fn dispatch(
    service: &Services,
    endpoint: Option<ExternalEndpoint>,
    request: ResourceRequest,
    file: Option<std::path::PathBuf>,
) -> Result<Option<Outcome>, ServiceError> {
    let docker = service.resource_docker(endpoint);
    service
        .pending(async move {
            let kind = match request {
                ResourceRequest::Run { id, name } => {
                    let name = name.map(ResourceName::parse).transpose()?;
                    return Ok(
                        match docker
                            .run_image(&ImageId::parse(id)?, name.as_ref())
                            .await?
                        {
                            ImageRun::Started(_) => Outcome::Success(SuccessKind::ContainerStarted),
                            ImageRun::Created { .. } => Outcome::ImageCreated,
                        },
                    );
                }
                ResourceRequest::PushImage { id, reference } => {
                    let current = docker.inspect_image(&ImageId::parse(id)?).await?;
                    if !current.repo_tags.iter().any(|tag| tag == &reference) {
                        return Err(marsa_client::docker::DockerError::ResourceIdentity.into());
                    }
                    docker
                        .push_image(&ImageReference::parse(reference)?)
                        .await?;
                    SuccessKind::ImagePushed
                }
                ResourceRequest::ImportImage => {
                    let Some(file) = file else {
                        return Ok(Outcome::Cancelled);
                    };
                    docker.import_image(&file).await?;
                    SuccessKind::ImageImported
                }
                ResourceRequest::ExportImage(id) => {
                    let Some(file) = file else {
                        return Ok(Outcome::Cancelled);
                    };
                    docker.export_image(&ImageId::parse(id)?, &file).await?;
                    SuccessKind::ImageExported
                }
                ResourceRequest::NetworkMembership {
                    network,
                    container,
                    attach,
                } => {
                    let network = NetworkId::parse(network)?;
                    if attach {
                        docker.connect_network(&network, &container).await?;
                        SuccessKind::ContainerAttached
                    } else {
                        docker.disconnect_network(&network, &container).await?;
                        SuccessKind::ContainerDetached
                    }
                }
                ResourceRequest::TagImage { id, destination } => {
                    docker.tag_image(&ImageId::parse(id)?, &destination).await?;
                    SuccessKind::ImageTagged
                }
                ResourceRequest::CreateVolume(name) => {
                    docker.create_volume(&ResourceName::parse(name)?).await?;
                    SuccessKind::VolumeCreated
                }
                ResourceRequest::CreateNetwork(name) => {
                    docker.create_network(&ResourceName::parse(name)?).await?;
                    SuccessKind::NetworkCreated
                }
                ResourceRequest::RemoveVolume { name, typed } => {
                    docker
                        .remove_volume(VolumeRemoval::confirm(ResourceName::parse(name)?, &typed)?)
                        .await?;
                    SuccessKind::VolumeRemoved
                }
                ResourceRequest::RemoveNetwork(id) => {
                    docker
                        .remove_network(NetworkRemoval::confirmed(id)?)
                        .await?;
                    SuccessKind::NetworkRemoved
                }
                ResourceRequest::RemoveImage { id, force } => {
                    let mode = if force {
                        RemovalMode::Force
                    } else {
                        RemovalMode::StoppedOnly
                    };
                    docker
                        .remove_image(ImageRemoval::confirmed(ImageId::parse(id)?, mode))
                        .await?;
                    SuccessKind::ImageRemoved
                }
                ResourceRequest::RemoveImages { ids, force } => {
                    let mode = if force {
                        RemovalMode::Force
                    } else {
                        RemovalMode::StoppedOnly
                    };
                    for id in ids {
                        docker
                            .remove_image(ImageRemoval::confirmed(ImageId::parse(id)?, mode))
                            .await?;
                    }
                    SuccessKind::ImagesRemoved
                }
                _ => return Ok(Outcome::Unavailable),
            };
            Ok(Outcome::Success(kind))
        })
        .result()
        .await
        .transpose()
}

impl ResourcesView {
    pub(crate) fn toast(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, outcome) = self.outcome?;
        let (message, tone) = match outcome {
            Outcome::Success(kind) => (kind.message(screen.locale), Tone::Success),
            Outcome::Rejected => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_DOCKER_REJECTED_THE_OPERATION_CHECK_THE_SELECTED_RESOURCE_AND,
                ),
                Tone::Error,
            ),
            Outcome::ImageCreated => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_THE_CONTAINER_WAS_CREATED_BUT_DOCKER_COULD_NOT_START_IT,
                ),
                Tone::Warning,
            ),
            Outcome::ComposeFileExists => (
                message(screen.locale, gui::GUI_COMPOSE_FILE_EXISTS),
                Tone::Warning,
            ),
            Outcome::ComposeFileWriteFailed => (
                message(screen.locale, gui::GUI_COMPOSE_FILE_WRITE_FAILED),
                Tone::Error,
            ),
            Outcome::ComposeFileKept => (
                message(screen.locale, gui::GUI_COMPOSE_FILE_KEPT),
                Tone::Warning,
            ),
            Outcome::ComposeKeychainFailed => (
                message(screen.locale, gui::GUI_DEPLOY_SECRETS_DEPLOY_FAILED),
                Tone::Error,
            ),
            Outcome::UnsupportedImagePlatform => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_THIS_IMAGE_HAS_NO_LINUX_ARM64_VARIANT_CHOOSE_AN_ARM64_OR_MULTI,
                ),
                Tone::Error,
            ),
            Outcome::Cancelled => (
                message(screen.locale, gui::GUI_DESKTOP_OPERATION_CANCELLED),
                Tone::Info,
            ),
            Outcome::Unavailable => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_THIS_OPERATION_IS_UNAVAILABLE,
                ),
                Tone::Warning,
            ),
        };
        let dismiss =
            toast::dismiss_button("dismiss-resource-toast", screen.locale, screen.palette)
                .on_click(cx.listener(|view, _, _, cx| {
                    cx.stop_propagation();
                    view.outcome = None;
                    cx.refresh_windows();
                }));
        Some(toast::render(
            ("resource-toast", outcome.toast_key()),
            screen.locale,
            screen.palette,
            tone,
            message,
            Some(dismiss),
            cx,
        ))
    }
}

pub(super) fn image_metadata(
    screen: &ScreenContext<'_>,
    row: &ResourceRow,
    view: &ResourcesView,
) -> Option<Div> {
    if view.inspection_task.is_none() && view.inspect.is_none() && !view.inspection_failed {
        return None;
    }
    let Some(image) = view.inspect.as_ref().filter(|image| image.id == row.id) else {
        let message = if view.inspection_failed {
            message(
                screen.locale,
                gui::GUI_DESKTOP_IMAGE_METADATA_IS_UNAVAILABLE,
            )
        } else {
            message(screen.locale, gui::GUI_DESKTOP_LOADING_IMAGE_METADATA)
        };
        return Some(
            text(screen, Role::Small, message).text_color(tone(screen, ColorToken::TextMuted)),
        );
    };
    let entries = [
        (
            message(screen.locale, gui::GUI_DESKTOP_PLATFORM),
            format!("{}/{}", image.os, image.architecture),
        ),
        (
            message(screen.locale, gui::GUI_DESKTOP_CREATED),
            image.created.clone().unwrap_or_else(|| {
                message(screen.locale, gui::GUI_DESKTOP_UNAVAILABLE).to_string()
            }),
        ),
    ];
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(Space::S4.pixels()))
            .children(entries.into_iter().map(|(name, value)| {
                logical_row(screen.locale)
                    .justify_between()
                    .child(text(screen, Role::Label, name))
                    .child(text(screen, Role::Mono, value))
            }))
            .children(
                image
                    .repo_digests
                    .iter()
                    .map(|digest| text(screen, Role::Mono, digest.clone()).truncate()),
            ),
    )
}
