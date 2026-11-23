//! Normalize a detected engine for the existing resource views without mixing its store with Marsa's.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::Arc,
};

use marsa_client::docker::{ExternalEndpoint, ExternalInventory};
use marsa_ui::{
    ColorToken, Palette,
    gpui::{Div, ParentElement as _, SharedString, Styled as _, div, px},
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
    pub(crate) volumes_available: bool,
    pub(crate) networks_available: bool,
}

impl From<ExternalInventory> for ExternalWorkspace {
    fn from(inventory: ExternalInventory) -> Self {
        Self {
            endpoint: inventory.endpoint,
            source: inventory.source,
            reachable: inventory.reachable,
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
            address: workspace.endpoint.address(),
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
            if name.as_ref() == workspace.source && !address.is_empty()
                && *address == workspace.endpoint.address())
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

#[cfg(test)]
mod tests {
    use super::*;

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
