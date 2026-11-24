//! Bounded pull progress and cancellation for the selected image download.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use std::{collections::BTreeMap, sync::Arc};

use marsa_client::docker::{DockerError, ExternalEndpoint, ImageReference, PullProgress};
use marsa_ui::{
    ColorToken,
    gpui::{Context, Div, Entity, ParentElement, Styled, div, px},
    gpui_component::{Disableable, button::Button, scroll::ScrollableElement as _},
    tokens::{scale::Space, typography::Role},
};

use crate::{
    screens::common::{ScreenContext, panel},
    services::{ServiceError, Services},
};

use super::{ResourceKind, ResourcesView, logical_row, operations::Outcome, text, tone};

const MAX_LAYERS: usize = 256;
const MAX_LABEL_BYTES: usize = 512;

pub(super) struct PullState {
    reference: String,
    layers: BTreeMap<String, PullProgress>,
}

impl ResourcesView {
    pub(super) fn pull(
        &mut self,
        reference: String,
        endpoint: Option<ExternalEndpoint>,
        service: Arc<Services>,
        cx: &mut Context<Self>,
    ) {
        let Ok(request) = ImageReference::parse(reference.clone()) else {
            self.outcome = Some((ResourceKind::Images, Outcome::Rejected));
            cx.refresh_windows();
            return;
        };
        self.busy = Some(ResourceKind::Images);
        self.outcome = None;
        self.pull = Some(PullState {
            reference,
            layers: BTreeMap::new(),
        });
        let external = endpoint.is_some();
        let mut stream = service.pull_image(request, endpoint);
        self.operation = Some(cx.spawn(async move |view, cx| {
            let mut outcome = Outcome::Success;
            while let Some(batch) = stream.next().await {
                let records = match batch {
                    Ok(records) => records,
                    Err(ServiceError::Docker(DockerError::ImagePlatform)) => {
                        outcome = Outcome::UnsupportedImagePlatform;
                        break;
                    }
                    Err(_) => {
                        outcome = Outcome::Rejected;
                        break;
                    }
                };
                let update = view.update(cx, |view, cx| {
                    let Some(pull) = &mut view.pull else {
                        return;
                    };
                    for mut record in records {
                        if let Some(status) = &mut record.status {
                            status.truncate(
                                status.floor_char_boundary(MAX_LABEL_BYTES.min(status.len())),
                            );
                        }
                        let id = record.id.as_deref().unwrap_or_default();
                        if id.len() <= MAX_LABEL_BYTES
                            && (pull.layers.len() < MAX_LAYERS || pull.layers.contains_key(id))
                        {
                            pull.layers.insert(id.to_owned(), record);
                        }
                    }
                    cx.refresh_windows();
                });
                if update.is_err() {
                    return;
                }
            }
            if !external && matches!(outcome, Outcome::Success) {
                let _ = service.refresh().result().await;
            }
            let _ = view.update(cx, |view, cx| {
                view.busy = None;
                if external {
                    cx.emit(super::ExternalResourcesChanged);
                }
                view.outcome = Some((ResourceKind::Images, outcome));
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }

    fn cancel_pull(&mut self, cx: &mut Context<Self>) {
        if self.busy != Some(ResourceKind::Images) || self.pull.is_none() {
            return;
        }
        self.operation = None;
        self.busy = None;
        self.outcome = Some((ResourceKind::Images, Outcome::Cancelled));
        cx.refresh_windows();
    }
}

pub(super) fn render(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    pull: Option<&PullState>,
    active: bool,
) -> Option<Div> {
    let pull = pull?;
    let view = view.clone();
    let heading = logical_row(screen.locale)
        .justify_between()
        .child(text(screen, Role::Mono, pull.reference.clone()))
        .child(
            Button::new("cancel-image-pull")
                .label(message(screen.locale, gui::GUI_DESKTOP_CANCEL_PULL))
                .disabled(!active)
                .on_click(move |_, _, cx| {
                    view.update(cx, |view, cx| view.cancel_pull(cx));
                }),
        );
    let layers = div().flex().flex_col().max_h(px(176.));
    let layers = layers.children(pull.layers.iter().map(|(layer, progress)| {
        let status = progress.status.as_deref().unwrap_or_default();
        let bytes = progress
            .progress
            .as_ref()
            .and_then(|counters| Some(format!("{} / {}", counters.current?, counters.total?)))
            .unwrap_or_default();
        logical_row(screen.locale)
            .flex_shrink_0()
            .gap(px(Space::S8.pixels()))
            .child(
                text(screen, Role::Mono, layer.clone())
                    .text_color(tone(screen, ColorToken::TextMuted)),
            )
            .child(text(screen, Role::Body, status.to_owned()).flex_1())
            .child(text(screen, Role::Mono, bytes))
    }));
    Some(
        panel(screen)
            .gap(px(Space::S8.pixels()))
            .child(heading)
            .children((!pull.layers.is_empty()).then(|| layers.overflow_y_scrollbar())),
    )
}
