//! Keep localizable validation reasons separate from legacy status messages.

use marsa_core::error::SettingsError;
use marsa_proto::host::v1::{ConfigurationIssue, configuration_issue::Reason};
use tonic::{Code, Status};

pub(super) fn reason(code: Code, reason: Reason, message: impl Into<String>) -> Status {
    ConfigurationIssue {
        reason: reason.into(),
        ..Default::default()
    }
    .status(code, message)
}

pub(super) fn settings_status(error: SettingsError) -> Status {
    let mut issue = ConfigurationIssue::default();
    let reason = match &error {
        SettingsError::TooLarge { limit } => {
            issue.size_limit = Some(*limit as u64);
            Reason::SettingsTooLarge
        }
        SettingsError::Decode { line, column } => {
            issue.line = Some(*line as u64);
            issue.column = Some(*column as u64);
            Reason::SettingsToml
        }
        SettingsError::UnsupportedVersion(version) => {
            issue.schema_version = Some(*version);
            Reason::SettingsUnsupportedVersion
        }
        SettingsError::InvalidValue { field, .. } => {
            issue.field = Some((*field).to_owned());
            match *field {
                "vm.cpus" => Reason::SettingsInvalidCpus,
                "vm.memory" => Reason::SettingsInvalidMemory,
                "vm.idle_timeout" => Reason::SettingsInvalidIdle,
                "disk.data_size_limit" => Reason::SettingsInvalidDiskLimit,
                "network.proxy.http" | "network.proxy.https" => Reason::SettingsInvalidProxy,
                "telemetry.enabled" => Reason::SettingsTelemetryUnavailable,
                "shares" => Reason::ShareInvalidPath,
                _ => Reason::SettingsInvalidValue,
            }
        }
        SettingsError::Encode => Reason::SettingsEncode,
        _ => Reason::SettingsInvalidValue,
    };
    issue.reason = reason.into();
    issue.status(Code::InvalidArgument, error.to_string())
}
