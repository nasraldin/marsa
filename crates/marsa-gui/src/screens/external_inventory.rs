//! Normalize a detected engine for the existing resource views without mixing its store with Marsa's.

use std::path::PathBuf;

use marsa_client::docker::ExternalInventory;
use marsa_ui::{
    ColorToken, Palette,
    gpui::{Div, ParentElement as _, SharedString, Styled as _, div, px},
};

use crate::{
    screens::common::color,
    store::{AppState, ResourceSnapshot},
};

pub(crate) struct ExternalWorkspace {
    pub(crate) socket: PathBuf,
    pub(crate) source: String,
    pub(crate) state: AppState,
    pub(crate) volumes_available: bool,
    pub(crate) networks_available: bool,
}

impl From<ExternalInventory> for ExternalWorkspace {
    fn from(inventory: ExternalInventory) -> Self {
        Self {
            socket: inventory.socket,
            source: inventory.source,
            volumes_available: inventory.volumes.is_some(),
            networks_available: inventory.networks.is_some(),
            state: AppState::index_resources(ResourceSnapshot {
                containers: inventory.containers,
                images: inventory.images,
                volumes: inventory.volumes.unwrap_or_default(),
                networks: inventory.networks.unwrap_or_default(),
            }),
        }
    }
}

pub(super) fn source_badge(palette: Palette, value: SharedString) -> Div {
    div()
        .max_w(px(100.))
        .flex_shrink_0()
        .truncate()
        .rounded(px(5.))
        .px_2()
        .text_xs()
        .bg(color(palette, ColorToken::AccentSubtle))
        .text_color(color(palette, ColorToken::TextPrimary))
        .child(value)
}
