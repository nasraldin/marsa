//! Dispatch resource commands and load selected image metadata outside the UI thread.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::sync::Arc;

use marsa_client::{
    compose::ComposeOperation,
    docker::{
        ContainerAction, ContainerId, ExternalEndpoint, ImageId, ImageRemoval, ImageRun,
        NetworkRemoval, RemovalMode, ResourceName, VolumeRemoval,
    },
};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, ParentElement, Styled, div, px},
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{ScreenContext, panel},
    services::{Command, ServiceError, Services},
};

use super::{
    ResourceKind, ResourceRequest, ResourcesView, compose::Invocation, logical_row,
    rows::ResourceRow, text, tone,
};

#[derive(Clone, Copy)]
pub(super) enum Outcome {
    Success,
    Rejected,
    Unavailable,
    ImageCreated,
    UnsupportedImagePlatform,
    Cancelled,
}

impl ResourcesView {
    pub(super) fn measure_volumes(&mut self, service: &Services, cx: &mut Context<Self>) {
        if self.volume_scanning {
            return;
        }
        self.volume_scanning = true;
        self.volume_scan_failed = false;
        let mut request = service.volume_usage();
        self.volume_scan = Some(cx.spawn(async move |view, cx| {
            let result = request.result().await;
            let _ = view.update(cx, |view, cx| {
                view.volume_scanning = false;
                match result {
                    Some(Ok(usage)) => view.volume_usage = usage,
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
        if self.selected_endpoint == endpoint
            && !self.reveal_selection
            && self
                .selection
                .as_ref()
                .is_some_and(|(selected, current)| *selected == kind && current == &key)
        {
            return;
        }
        self.selection = Some((kind, key.clone()));
        self.selected_endpoint = endpoint.clone();
        self.reveal_selection = false;
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
                Outcome::Success
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
            ResourceRequest::Pull(reference) => {
                self.pull(reference, endpoint, service, cx);
                return;
            }
            ResourceRequest::ComposeUp(project) => {
                self.run_compose(
                    project,
                    Invocation::Standard(ComposeOperation::Up),
                    service,
                    locale,
                    cx,
                );
                return;
            }
            ResourceRequest::ComposeRestart(project) => {
                self.run_compose(
                    project,
                    Invocation::Standard(ComposeOperation::Restart),
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
                self.run_compose(project, invocation, service, locale, cx);
                return;
            }
            request => request,
        };
        self.pull = None;
        self.busy = Some(kind);
        self.outcome = None;
        self.operation = Some(cx.spawn(async move |view, cx| {
            let external = endpoint.is_some();
            let result = match endpoint {
                Some(endpoint) => dispatch_external(&service, endpoint, request).await,
                None => dispatch(&service, request).await,
            };
            let outcome = match result {
                Ok(Some(outcome)) => {
                    if !external {
                        let _ = service.refresh().result().await;
                    }
                    outcome
                }
                Ok(None) => Outcome::Unavailable,
                Err(_) => Outcome::Rejected,
            };
            let _ = view.update(cx, |view, cx| {
                view.busy = None;
                if kind == ResourceKind::Volumes && matches!(outcome, Outcome::Success) {
                    view.volume_usage.clear();
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

async fn dispatch_external(
    service: &Services,
    endpoint: ExternalEndpoint,
    request: ResourceRequest,
) -> Result<Option<Outcome>, ServiceError> {
    let docker = endpoint.management_client();
    service
        .pending(async move {
            match request {
                ResourceRequest::Run { id, name } => {
                    let name = name.map(ResourceName::parse).transpose()?;
                    return Ok(
                        match docker
                            .run_image(&ImageId::parse(id)?, name.as_ref())
                            .await?
                        {
                            ImageRun::Started(_) => Outcome::Success,
                            ImageRun::Created { .. } => Outcome::ImageCreated,
                        },
                    );
                }
                ResourceRequest::TagImage { id, destination } => {
                    docker.tag_image(&ImageId::parse(id)?, &destination).await?;
                }
                ResourceRequest::CreateVolume(name) => {
                    docker.create_volume(&ResourceName::parse(name)?).await?;
                }
                ResourceRequest::CreateNetwork(name) => {
                    docker.create_network(&ResourceName::parse(name)?).await?;
                }
                ResourceRequest::RemoveVolume { name, typed } => {
                    docker
                        .remove_volume(VolumeRemoval::confirm(ResourceName::parse(name)?, &typed)?)
                        .await?;
                }
                ResourceRequest::RemoveNetwork(id) => {
                    docker
                        .remove_network(NetworkRemoval::confirmed(id)?)
                        .await?;
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
                }
                _ => return Ok(Outcome::Unavailable),
            }
            Ok(Outcome::Success)
        })
        .result()
        .await
        .transpose()
}

async fn dispatch(
    service: &Services,
    request: ResourceRequest,
) -> Result<Option<Outcome>, ServiceError> {
    if let ResourceRequest::Run { id, name } = request {
        let name = name.map(ResourceName::parse).transpose()?;
        let result = service
            .run_image(ImageId::parse(id)?, name)
            .result()
            .await
            .transpose()?;
        return Ok(result.map(|result| match result {
            ImageRun::Started(_) => Outcome::Success,
            ImageRun::Created { .. } => Outcome::ImageCreated,
        }));
    }
    let result = match request {
        ResourceRequest::TagImage { id, destination } => {
            service
                .tag_image(ImageId::parse(id)?, destination)
                .result()
                .await
        }
        ResourceRequest::CreateVolume(name) => service
            .create_volume(ResourceName::parse(name)?)
            .result()
            .await
            .map(|result| result.map(|_| ())),
        ResourceRequest::CreateNetwork(name) => service
            .create_network(ResourceName::parse(name)?)
            .result()
            .await
            .map(|result| result.map(|_| ())),
        ResourceRequest::RemoveVolume { name, typed } => {
            let removal = VolumeRemoval::confirm(ResourceName::parse(name)?, &typed)?;
            service
                .dispatch(Command::RemoveVolume(removal))
                .result()
                .await
        }
        ResourceRequest::RemoveNetwork(id) => {
            let removal = NetworkRemoval::confirmed(id)?;
            service
                .dispatch(Command::RemoveNetwork(removal))
                .result()
                .await
        }
        ResourceRequest::RemoveImage { id, force } => {
            let mode = if force {
                RemovalMode::Force
            } else {
                RemovalMode::StoppedOnly
            };
            let removal = ImageRemoval::confirmed(ImageId::parse(id)?, mode);
            service
                .dispatch(Command::RemoveImage(removal))
                .result()
                .await
        }
        ResourceRequest::Pull(_)
        | ResourceRequest::Run { .. }
        | ResourceRequest::ComposeUp(_)
        | ResourceRequest::ComposeRestart(_)
        | ResourceRequest::ComposeDown { .. } => None,
    };
    result
        .transpose()
        .map(|result| result.map(|()| Outcome::Success))
}

pub(super) fn notice(
    screen: &ScreenContext<'_>,
    kind: ResourceKind,
    view: &ResourcesView,
) -> Option<Div> {
    let (message, role) = if view.busy == Some(kind) {
        (
            message(screen.locale, gui::GUI_DESKTOP_WORKING),
            ColorToken::TextMuted,
        )
    } else {
        match view.outcome.filter(|(page, _)| *page == kind)?.1 {
            Outcome::Success => (
                message(screen.locale, gui::GUI_DESKTOP_OPERATION_COMPLETED),
                ColorToken::StatusRunning,
            ),
            Outcome::Rejected => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_DOCKER_REJECTED_THE_OPERATION_CHECK_THE_SELECTED_RESOURCE_AND,
                ),
                ColorToken::StatusError,
            ),
            Outcome::ImageCreated => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_THE_CONTAINER_WAS_CREATED_BUT_DOCKER_COULD_NOT_START_IT,
                ),
                ColorToken::StatusWarning,
            ),
            Outcome::UnsupportedImagePlatform => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_THIS_IMAGE_HAS_NO_LINUX_ARM64_VARIANT_CHOOSE_AN_ARM64_OR_MULTI,
                ),
                ColorToken::StatusError,
            ),
            Outcome::Cancelled => (
                message(screen.locale, gui::GUI_DESKTOP_IMAGE_PULL_CANCELLED),
                ColorToken::TextMuted,
            ),
            Outcome::Unavailable => (
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_THIS_OPERATION_IS_UNAVAILABLE,
                ),
                ColorToken::StatusWarning,
            ),
        }
    };
    Some(panel(screen).child(text(screen, Role::Body, message).text_color(tone(screen, role))))
}

pub(super) fn image_metadata(
    screen: &ScreenContext<'_>,
    row: &ResourceRow,
    view: &ResourcesView,
) -> Option<Div> {
    if view.inspection_task.is_none() && !view.inspection_failed {
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
