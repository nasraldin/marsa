//! Normalize a detected engine for the existing resource views without mixing its store with Marsa's.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::Arc,
};

use marsa_client::docker::{ExternalEndpoint, ExternalInventory, LimaSettings, VerifiedEngine};
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{Div, FontWeight, ParentElement as _, SharedString, Styled as _, div, px},
    gui_keys as gui,
    tokens::{scale::Space, typography::ui_size},
};

use crate::{
    screens::common::color,
    store::{AppState, ResourceSnapshot},
};

#[derive(Clone)]
pub(crate) struct ExternalWorkspace {
    pub(crate) endpoint: ExternalEndpoint,
    pub(crate) source: String,
    pub(crate) state: AppState,
    pub(crate) reachable: bool,
    pub(crate) verified_engine: Option<VerifiedEngine>,
    pub(crate) lima_settings: Option<LimaSettings>,
    pub(crate) volumes_available: bool,
    pub(crate) networks_available: bool,
}

impl From<ExternalInventory> for ExternalWorkspace {
    fn from(inventory: ExternalInventory) -> Self {
        Self {
            endpoint: inventory.endpoint,
            source: inventory.source,
            reachable: inventory.reachable,
            verified_engine: inventory.verified_engine,
            lima_settings: inventory.lima_settings,
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

pub(crate) type Workspaces = Arc<[Arc<ExternalWorkspace>]>;

/// Identity of the environment selected by the desktop.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum Environment {
    #[default]
    Marsa,
    External {
        name: Arc<str>,
        address: String,
    },
}

impl From<&ExternalWorkspace> for Environment {
    fn from(workspace: &ExternalWorkspace) -> Self {
        Self::External {
            name: Arc::from(workspace.source.as_str()),
            address: workspace
                .endpoint
                .managed_id()
                .map_or_else(|| workspace.endpoint.address(), str::to_owned),
        }
    }
}

impl Environment {
    const MAX_PREFERENCE_BYTES: u64 = 8192;

    fn preference_path() -> Option<PathBuf> {
        let socket = marsa_client::docker_socket_path().ok()?;
        Some(socket.parent()?.parent()?.join("desktop-environment"))
    }

    pub(crate) fn load() -> Self {
        let mut contents = String::new();
        if Self::preference_path()
            .and_then(|path| File::open(path).ok())
            .is_some_and(|file| {
                file.take(Self::MAX_PREFERENCE_BYTES + 1)
                    .read_to_string(&mut contents)
                    .is_ok()
            })
        {
            return Self::decode(&contents);
        }
        Self::Marsa
    }

    fn decode(contents: &str) -> Self {
        if contents.len() as u64 <= Self::MAX_PREFERENCE_BYTES
            && let Some((name, address)) = contents.split_once('\n')
            && !name.is_empty()
            && !address.is_empty()
            && !name.chars().chain(address.chars()).any(char::is_control)
        {
            return Self::External {
                name: Arc::from(name),
                address: address.into(),
            };
        }
        Self::Marsa
    }

    pub(crate) fn save(&self) {
        let Some(path) = Self::preference_path() else {
            return;
        };
        let contents = match self {
            Self::Marsa => String::new(),
            Self::External { name, address } => format!("{name}\n{address}"),
        };
        if !self.includes_marsa() && Self::decode(&contents) != *self {
            return;
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        if let Ok(mut file) = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            && (file.write_all(contents.as_bytes()).is_err()
                || fs::rename(&temporary, path).is_err())
        {
            let _ = fs::remove_file(temporary);
        }
    }

    pub(crate) fn includes_marsa(&self) -> bool {
        matches!(self, Self::Marsa)
    }

    pub(crate) fn includes_external(&self, workspace: &ExternalWorkspace) -> bool {
        matches!(self, Self::External { name, address }
        if workspace.endpoint.managed_id().map_or_else(
            || name.as_ref() == workspace.source && !address.is_empty()
                && *address == workspace.endpoint.address(),
            |id| address == id,
        ))
    }

    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self == other
            || matches!((self, other),
            (Self::External { address, .. }, Self::External { address: other, .. })
            if address.starts_with("managed:") && address == other)
    }

    pub(crate) fn settings_key(&self) -> String {
        match self {
            Self::Marsa => "marsa".to_owned(),
            Self::External { address, .. } if address.starts_with("managed:") => address.clone(),
            Self::External { name, address } => {
                format!("discovered:{}:{name}:{address}", name.len())
            }
        }
    }

    /// The unformatted environment name for renderer-owned text isolation.
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Marsa => "Marsa",
            Self::External { name, .. } => name,
        }
    }

    pub(crate) fn display(&self) -> SharedString {
        match self {
            Self::Marsa => "Marsa".into(),
            Self::External { name, .. } => super::common::technical(name),
        }
    }
}

pub(super) fn source_badge(palette: Palette, value: SharedString) -> Div {
    div()
        .max_w(px(144.))
        .flex_shrink_0()
        .truncate()
        .rounded(px(5.))
        .px_2()
        .text_xs()
        .bg(color(palette, ColorToken::AccentSubtle))
        .text_color(color(palette, ColorToken::TextPrimary))
        .child(value)
}

/// Show the full pinned environment on an operation before the user confirms it.
pub(super) fn dialog_scope(locale: Locale, palette: Palette, name: SharedString) -> Div {
    super::common::align(
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(Space::S4.pixels()))
            .border_b_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .pb(px(Space::S12.pixels())),
        locale,
    )
    .child(
        div()
            .text_size(ui_size(12.))
            .text_color(color(palette, ColorToken::TextMuted))
            .child(gui::GUI_WORKLOAD_TARGET_ENVIRONMENT.text(locale)),
    )
    .child(
        div()
            .text_size(ui_size(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color(palette, ColorToken::TextPrimary))
            .child(name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_identity_survives_rename_without_matching_a_different_endpoint() {
        let before = Environment::decode("Lima\nmanaged:one");
        let renamed = Environment::decode("Development\nmanaged:one");
        let replaced = Environment::decode("Lima\nmanaged:two");
        assert!(before.same_identity(&renamed));
        assert!(!before.same_identity(&replaced));
        assert_eq!(before.settings_key(), renamed.settings_key());
        assert_ne!(before.settings_key(), replaced.settings_key());
        assert_ne!(before.settings_key(), Environment::Marsa.settings_key());
        assert_ne!(
            Environment::decode("Lima\n/tmp/one.sock").settings_key(),
            Environment::decode("Lima\n/tmp/two.sock").settings_key()
        );
        assert!(
            !Environment::decode("Lima\n/tmp/docker.sock")
                .same_identity(&Environment::decode("Development\n/tmp/docker.sock"))
        );
    }

    #[test]
    fn saved_environment_requires_an_address() {
        let saved = Environment::decode("lima\n/Users/example/.lima/docker/sock/docker.sock");
        assert!(matches!(saved, Environment::External { name, .. } if name.as_ref() == "lima"));
        for invalid in ["", "lima", "lima\n", "lima\n/one\n/two", "\n/one"] {
            assert!(Environment::decode(invalid).includes_marsa());
        }
        assert!(Environment::decode(&"a".repeat(8193)).includes_marsa());
    }
}
