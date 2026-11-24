//! Application metadata carried in an authenticated TUF target descriptor.
//!
//! The updater verifies TUF's signatures, size and hashes before consuming
//! these fields. This module validates Marsa's portion of that format.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::settings::UpdateChannel;

const SCHEMA_VERSION: u32 = 1;
const MAX_NOTES_BYTES: usize = 8 * 1024;
const MINIMUM_MACOS: Version = Version(27, 0, 0);

/// A release version in canonical `MAJOR.MINOR.PATCH` notation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Version(u32, u32, u32);

impl FromStr for Version {
    type Err = ManifestError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split('.');
        let mut next = || {
            let part = parts.next().ok_or(ManifestError::InvalidVersion)?;
            if part.is_empty()
                || (part.starts_with('0') && part.len() > 1)
                || !part.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(ManifestError::InvalidVersion);
            }
            part.parse::<u32>()
                .map_err(|_| ManifestError::InvalidVersion)
        };
        let version = Self(next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(ManifestError::InvalidVersion);
        }
        Ok(version)
    }
}

impl TryFrom<String> for Version {
    type Error = ManifestError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Version> for String {
    fn from(value: Version) -> Self {
        value.to_string()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(output, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Localized release notes displayed as plain text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNotes {
    /// American English release notes.
    #[serde(rename = "en-US")]
    pub english: String,
    /// Modern Standard Arabic release notes.
    #[serde(rename = "ar")]
    pub arabic: String,
}

impl ReleaseNotes {
    fn valid(&self) -> bool {
        [&self.english, &self.arabic].into_iter().all(|notes| {
            !notes.trim().is_empty()
                && notes.len() <= MAX_NOTES_BYTES
                && !notes
                    .chars()
                    .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        })
    }
}

/// Marsa-specific metadata for one immutable macOS application archive.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMetadata {
    schema_version: u32,
    version: Version,
    minimum_macos: Version,
    channels: Vec<UpdateChannel>,
    security_recommended: bool,
    notes: ReleaseNotes,
}

/// Invalid application metadata within a target descriptor.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// Unknown fields, missing fields or wrong JSON types.
    #[error("invalid update release metadata")]
    Decode(#[source] serde_json::Error),
    /// The caller needs a client that understands this schema before using it.
    #[error("unsupported update metadata schema {0}")]
    Schema(u32),
    /// A version uses an unsupported representation or exceeds a numeric limit.
    #[error("expected a canonical MAJOR.MINOR.PATCH version")]
    InvalidVersion,
    /// The metadata contradicts Marsa's supported macOS baseline.
    #[error("invalid minimum macOS version")]
    MinimumMacos,
    /// At least one channel is required and each may appear once.
    #[error("invalid update channel membership")]
    Channels,
    /// The notes are missing a required locale or contain unsupported text.
    #[error("invalid localized update notes")]
    Notes,
    /// The authenticated TUF name does not match the release version and platform.
    #[error("update target name does not match release metadata")]
    TargetName,
}

impl ReleaseMetadata {
    /// Build validated metadata for publication as one target's custom fields.
    pub fn new(
        version: Version,
        minimum_macos: Version,
        channels: Vec<UpdateChannel>,
        security_recommended: bool,
        notes: ReleaseNotes,
    ) -> Result<Self, ManifestError> {
        let release = Self {
            schema_version: SCHEMA_VERSION,
            version,
            minimum_macos,
            channels,
            security_recommended,
            notes,
        };
        release.validate()?;
        Ok(release)
    }

    /// Validate the Marsa fields of a TUF target whose signature has been checked.
    pub fn from_verified_target(
        name: &str,
        custom: serde_json::Value,
    ) -> Result<Self, ManifestError> {
        let release: Self = serde_json::from_value(custom).map_err(ManifestError::Decode)?;
        release.validate()?;
        if name != release.target_name() {
            return Err(ManifestError::TargetName);
        }
        Ok(release)
    }

    /// Return the version of the app in the archive.
    pub fn version(&self) -> Version {
        self.version
    }

    /// Return the oldest macOS version able to run this artifact.
    pub fn minimum_macos(&self) -> Version {
        self.minimum_macos
    }

    /// Check whether this exact artifact was published to the selected channel.
    pub fn includes_channel(&self, channel: UpdateChannel) -> bool {
        self.channels.contains(&channel)
    }

    /// Return whether an available update should be highlighted for security reasons.
    pub fn security_recommended(&self) -> bool {
        self.security_recommended
    }

    /// Return the localized plain-text notes for this build.
    pub fn notes(&self) -> &ReleaseNotes {
        &self.notes
    }

    /// Return the canonical TUF target name for the macOS Apple silicon archive.
    pub fn target_name(&self) -> String {
        format!("releases/marsa-{}-darwin-arm64.zip", self.version)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ManifestError::Schema(self.schema_version));
        }
        if self.minimum_macos < MINIMUM_MACOS {
            return Err(ManifestError::MinimumMacos);
        }
        let channels_valid = match self.channels.as_slice() {
            [_] => true,
            [first, second] => first != second,
            _ => false,
        };
        if !channels_valid {
            return Err(ManifestError::Channels);
        }
        if !self.notes.valid() {
            return Err(ManifestError::Notes);
        }
        Ok(())
    }
}
