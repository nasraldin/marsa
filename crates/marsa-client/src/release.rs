//! Read the installed release's local metadata without depending on a daemon.

use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{api::ComponentVersion, l10n::Locale};

const RELEASE_NOTES_LIMIT: usize = 32 * 1024;
const UPSTREAM_INVENTORY_LIMIT: usize = 4 * 1024 * 1024;

/// Files a user can inspect to verify the installed release inventory.
#[derive(Clone, Copy)]
pub enum Evidence {
    /// Inventory of upstream Docker command-line tools.
    HostTools,
    /// Distribution inventory for Marsa and its dependencies.
    AppSbom,
    /// Distribution inventory for the bundled guest image.
    GuestSbom,
    /// Distribution copyright and licence notices.
    Notices,
}

impl Evidence {
    fn relative_path(self) -> &'static str {
        match self {
            Self::HostTools => "licenses/upstream-sbom-input.json",
            Self::AppSbom => "licenses/marsa-sbom.cdx.json",
            Self::GuestSbom => "licenses/guest-sbom.cdx.json",
            Self::Notices => "licenses/THIRD-PARTY-NOTICES.txt",
        }
    }
}

/// Dependency information and notes supplied by the installed app bundle.
#[derive(Default)]
pub struct AboutAssets {
    notes: Option<ReleaseNotes>,
    host_tools: Vec<ComponentVersion>,
    evidence: [Option<PathBuf>; 4],
}

impl AboutAssets {
    /// Read the installed metadata from a trusted app bundle's Resources directory.
    ///
    /// Missing or malformed resources are independently unavailable. Notes
    /// for a different version are never presented for this build.
    pub fn read(resources: &Path, version: &str) -> Self {
        let notes =
            read_json::<ReleaseNotes>(&resources.join("release-notes.json"), RELEASE_NOTES_LIMIT)
                .filter(|notes| {
                    notes.version == version
                        && valid_notes(&notes.english)
                        && valid_notes(&notes.arabic)
                });
        let host_tools = read_json::<Inventory>(
            &resources.join(Evidence::HostTools.relative_path()),
            UPSTREAM_INVENTORY_LIMIT,
        )
        .map_or_else(Vec::new, |inventory| {
            inventory
                .components
                .into_iter()
                .filter_map(|entry| {
                    let version = entry.version?;
                    if !technical_value(&entry.name) || !technical_value(&version) {
                        return None;
                    }
                    Some(ComponentVersion {
                        name: entry.name,
                        version,
                    })
                })
                .collect()
        });
        let evidence = [
            Evidence::HostTools,
            Evidence::AppSbom,
            Evidence::GuestSbom,
            Evidence::Notices,
        ]
        .map(|kind| {
            let path = resources.join(kind.relative_path());
            regular_file(&path).then_some(path)
        });
        Self {
            notes,
            host_tools,
            evidence,
        }
    }

    /// Return the release notes in the selected application language.
    pub fn notes(&self, locale: Locale) -> Option<&str> {
        let notes = self.notes.as_ref()?;
        Some(match locale {
            Locale::EnglishUs => &notes.english,
            Locale::Arabic => &notes.arabic,
        })
    }

    /// Return the versions recorded in the bundled Docker tools inventory.
    pub fn host_tools(&self) -> &[ComponentVersion] {
        &self.host_tools
    }

    /// Return the installed path when this piece of evidence is available.
    pub fn evidence(&self, kind: Evidence) -> Option<&Path> {
        self.evidence[kind as usize].as_deref()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseNotes {
    version: String,
    #[serde(rename = "en-US")]
    english: String,
    #[serde(rename = "ar")]
    arabic: String,
}

#[derive(Deserialize)]
struct Inventory {
    components: Vec<InventoryEntry>,
}

#[derive(Deserialize)]
struct InventoryEntry {
    name: String,
    version: Option<String>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: usize) -> Option<T> {
    if !regular_file(path) {
        return None;
    }
    let mut contents = Vec::new();
    File::open(path)
        .ok()?
        .take(limit as u64 + 1)
        .read_to_end(&mut contents)
        .ok()?;
    if contents.len() > limit {
        return None;
    }
    serde_json::from_slice(&contents).ok()
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn valid_notes(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 8 * 1024
        && !text
            .chars()
            .any(|character| character.is_control() && character != '\n')
}

fn technical_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_graphic())
}
