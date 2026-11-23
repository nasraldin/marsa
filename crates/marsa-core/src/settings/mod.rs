//! Host settings validation and versioned TOML persistence format.

mod network;
mod preferences;
mod vm;

pub use network::{NetworkMode, NetworkSettings, ProxySettings};
pub use preferences::{
    AccentColor, AgentSettings, DownloadFormat, Language, LogBufferSize, LogSettings,
    NotificationSettings, OperateApproval, TelemetrySettings, Theme, UiSettings, UpdateChannel,
    UpdateSettings, valid_compose_template,
};
pub use vm::{DiskSettings, VmBackend, VmSettings};

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::SettingsError;
use crate::paths::validate_directory;

/// Current schema written by the daemon.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Upper bound on a settings document's UTF-8 byte length.
pub const MAX_SETTINGS_BYTES: usize = 256 * 1024;

/// Shared configuration settings stored in the daemon's TOML document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    schema_version: u32,
    /// Absolute host paths the user has chosen to share.
    pub shares: Vec<PathBuf>,
    /// Virtual machine settings.
    pub vm: VmSettings,
    /// Data disk limits and reclaim behavior.
    pub disk: DiskSettings,
    /// Guest network and proxy overrides.
    pub network: NetworkSettings,
    /// Application language and appearance.
    pub ui: UiSettings,
    /// Updater preferences.
    pub updates: UpdateSettings,
    /// Agent permission defaults.
    pub agents: AgentSettings,
    /// Local privacy settings.
    pub telemetry: TelemetrySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            shares: Vec::new(),
            vm: VmSettings::default(),
            disk: DiskSettings::default(),
            network: NetworkSettings::default(),
            ui: UiSettings::default(),
            updates: UpdateSettings::default(),
            agents: AgentSettings::default(),
            telemetry: TelemetrySettings::default(),
        }
    }
}

impl Settings {
    /// Deserializes, migrates and validates a local TOML configuration document.
    pub fn from_toml(document: &str) -> Result<Self, SettingsError> {
        check_size(document)?;
        let header: SchemaHeader = decode(document)?;
        let version = header.schema_version.unwrap_or(0);
        if version != 0 && version != i64::from(SETTINGS_SCHEMA_VERSION) {
            return Err(SettingsError::UnsupportedVersion(version));
        }

        let mut settings: Self = decode(document)?;
        if version == 0 {
            // The draft schema used the same fields without a required version key.
            settings.schema_version = SETTINGS_SCHEMA_VERSION;
        }
        settings.validate()?;
        Ok(settings)
    }

    /// Validates and serializes the configuration using the current schema.
    pub fn to_toml(&self) -> Result<String, SettingsError> {
        self.validate()?;
        let document = toml::to_string_pretty(self).map_err(|_| SettingsError::Encode)?;
        check_size(&document)?;
        Ok(document)
    }

    /// Checks settings values before the daemon applies or persists them.
    ///
    /// Host resource limits and share permissions are enforced when those resources are used.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            return Err(SettingsError::UnsupportedVersion(i64::from(
                self.schema_version,
            )));
        }
        self.vm.validate()?;
        self.disk.validate()?;
        self.network.validate()?;
        if self
            .ui
            .compose_template
            .as_deref()
            .is_some_and(|value| !valid_compose_template(value))
        {
            return Err(SettingsError::invalid(
                "ui.compose_template",
                "must be nonblank, at most 16 KiB, and contain no NUL bytes",
            ));
        }
        if self
            .shares
            .iter()
            .any(|path| path.to_str().is_none() || validate_directory(path).is_err())
        {
            return Err(SettingsError::invalid(
                "shares",
                "paths must be absolute, non-root UTF-8 paths without parent traversal or NUL bytes",
            ));
        }
        if self.telemetry.enabled {
            return Err(SettingsError::invalid(
                "telemetry.enabled",
                "must be false because telemetry is unavailable",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct SchemaHeader {
    schema_version: Option<i64>,
}

fn check_size(document: &str) -> Result<(), SettingsError> {
    if document.len() > MAX_SETTINGS_BYTES {
        return Err(SettingsError::TooLarge {
            limit: MAX_SETTINGS_BYTES,
        });
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(document: &str) -> Result<T, SettingsError> {
    toml::from_str(document).map_err(|error| {
        // Keep the parser's position but discard its source excerpt and untrusted strings.
        let offset = error.span().map_or(0, |span| span.start);
        let prefix = document.floor_char_boundary(offset.min(document.len()));
        let mut lines = document[..prefix].split('\n');
        let line = lines.clone().count();
        let column = lines.next_back().map_or(1, |line| line.chars().count() + 1);
        SettingsError::Decode { line, column }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_view_preferences_keep_old_defaults_and_round_trip() {
        let legacy = Settings::from_toml("[ui]\nlanguage = \"en-US\"").unwrap();
        assert_eq!(legacy.ui.logs.buffer.container_lines(), 8_000);
        assert_eq!(legacy.ui.logs.buffer.stack_lines(), 256);
        assert!(!legacy.ui.logs.local_timestamps);

        let settings = Settings::from_toml(
            "schema_version = 1\n[ui.logs]\nbuffer = \"compact\"\nlocal_timestamps = true",
        )
        .unwrap();
        assert_eq!(settings.ui.logs.buffer.container_lines(), 500);
        assert_eq!(settings.ui.logs.buffer.stack_lines(), 64);
        assert!(settings.ui.logs.local_timestamps);
        assert_eq!(
            Settings::from_toml(&settings.to_toml().unwrap()).unwrap(),
            settings
        );
        assert!(Settings::from_toml("[ui.logs]\nbuffer = \"unlimited\"").is_err());
    }

    #[test]
    fn document_limit_is_checked_before_the_parser_runs() {
        let oversized = " ".repeat(MAX_SETTINGS_BYTES + 1);
        assert_eq!(
            Settings::from_toml(&oversized),
            Err(SettingsError::TooLarge {
                limit: MAX_SETTINGS_BYTES,
            }),
        );
    }

    #[test]
    fn decode_error_points_to_the_failing_line() {
        let error = Settings::from_toml("# تعليق\n[vm]\ncpus = \"invalid\"").unwrap_err();
        assert!(matches!(
            error,
            SettingsError::Decode { line: 3, column: 8 }
        ));
    }
}
