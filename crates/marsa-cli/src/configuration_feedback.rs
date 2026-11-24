//! Render typed settings and sharing rejections from the authenticated daemon.

use marsa_client::{
    api::{ConfigurationIssue, configuration_issue::Reason},
    l10n::keys,
};

use crate::{Outcome, command};

pub(super) fn rejection(
    issue: Option<ConfigurationIssue>,
    legacy_message: Option<&str>,
) -> Outcome {
    let detail = issue.as_ref().and_then(configuration_detail);
    let message = with_detail(detail.as_deref());
    let text = match (detail, legacy_message) {
        (None, Some(legacy)) => {
            let escaped = command::technical(&legacy.chars().take(512).collect::<String>());
            with_detail(Some(&escaped))
        }
        _ => message.clone(),
    };
    Outcome::Rejected { text, message }
}

fn with_detail(detail: Option<&str>) -> String {
    match detail {
        Some(detail) => command::message(
            keys::CLI_SETTINGS_REJECTED_WITH_DETAIL,
            &[("detail", detail)],
        ),
        None => command::label(keys::CLI_SETTINGS_REJECTED).to_owned(),
    }
}

fn configuration_detail(issue: &ConfigurationIssue) -> Option<String> {
    let key = match issue.reason() {
        Reason::SettingsTooLarge => {
            let bytes = command::technical(&issue.size_limit?.to_string());
            return Some(command::message(
                keys::CLI_CONFIG_TOO_LARGE,
                &[("bytes", &bytes)],
            ));
        }
        Reason::SettingsToml => {
            let line = command::technical(&issue.line?.to_string());
            let column = command::technical(&issue.column?.to_string());
            return Some(command::message(
                keys::CLI_CONFIG_INVALID_TOML,
                &[("line", &line), ("column", &column)],
            ));
        }
        Reason::SettingsUnsupportedVersion => {
            let version = command::technical(&issue.schema_version?.to_string());
            return Some(command::message(
                keys::CLI_CONFIG_VERSION,
                &[("version", &version)],
            ));
        }
        Reason::SettingsInvalidCpus => keys::CLI_CONFIG_CPUS,
        Reason::SettingsInvalidMemory => keys::CLI_CONFIG_MEMORY,
        Reason::SettingsInvalidIdle => keys::CLI_CONFIG_IDLE,
        Reason::SettingsInvalidDiskLimit => keys::CLI_CONFIG_DISK_LIMIT,
        Reason::SettingsInvalidProxy => keys::CLI_CONFIG_PROXY,
        Reason::SettingsTelemetryUnavailable => keys::CLI_CONFIG_TELEMETRY,
        Reason::SettingsInvalidValue => keys::CLI_CONFIG_INVALID,
        Reason::SettingsStale => return Some(command::label(keys::CLI_CONFIG_STALE).into()),
        Reason::SettingsProxyEnabled => {
            return Some(command::label(keys::CLI_CONFIG_PROXY_ENABLED).into());
        }
        Reason::SettingsEncode => return Some(command::label(keys::CLI_CONFIG_ENCODE).into()),
        Reason::ShareInvalidPath => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_PATH).into());
        }
        Reason::ShareResolutionFailed => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_RESOLVE).into());
        }
        Reason::ShareProtectionUnverified => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_PROTECTION).into());
        }
        Reason::ShareProtected => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_PROTECTED).into());
        }
        Reason::ShareNotDirectory => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_DIRECTORY).into());
        }
        Reason::ShareUnsupportedSource => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_SOURCE).into());
        }
        Reason::ShareNotAllowed => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_NOT_ALLOWED).into());
        }
        Reason::ShareGuestLayout => {
            return Some(command::label(keys::CLI_CONFIG_SHARE_LAYOUT).into());
        }
        Reason::ShareUnknown => return Some(command::label(keys::CLI_CONFIG_SHARE_UNKNOWN).into()),
        Reason::Unspecified => return None,
    };
    let field = command::technical(issue.field.as_deref()?);
    Some(command::message(key, &[("field", &field)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_actionable_typed_details() {
        let issue = ConfigurationIssue {
            reason: Reason::SettingsInvalidCpus.into(),
            field: Some("vm.cpus".into()),
            ..Default::default()
        };
        let Outcome::Rejected { text: english, .. } = rejection(Some(issue), Some("old text"))
        else {
            panic!("expected a rejection")
        };
        assert_eq!(
            english,
            "The Marsa daemon rejected the request. invalid setting vm.cpus: must be at least one"
        );
    }

    #[test]
    fn a_current_peer_without_recognized_details_uses_local_catalog_copy() {
        let Outcome::Rejected { text, .. } = rejection(None, None) else {
            panic!("expected a rejection")
        };
        assert_eq!(text, "The Marsa daemon rejected the request.");
    }

    #[test]
    fn legacy_arabic_status_is_human_only_and_keeps_json_english() {
        let Outcome::Rejected { text, message } = rejection(None, Some("رسالة قديمة"))
        else {
            panic!("expected a rejection")
        };
        assert!(text.contains("رسالة قديمة"));
        assert_eq!(message, "The Marsa daemon rejected the request.");
    }

    #[test]
    fn a_legacy_peer_still_gets_a_safe_actionable_diagnostic() {
        let Outcome::Rejected { text, .. } = rejection(None, Some("bad\nvalue\u{202e}")) else {
            panic!("expected a rejection")
        };
        assert_eq!(
            text,
            r"The Marsa daemon rejected the request. bad\nvalue\u{202e}"
        );
    }
}
