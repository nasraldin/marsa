//! Localize typed settings and sharing rejections from the authenticated daemon.

use marsa_client::{
    api::{ConfigurationIssue, configuration_issue::Reason},
    l10n::keys,
};

use crate::{Outcome, command::Language};

pub(super) fn rejection(
    language: Language,
    issue: Option<ConfigurationIssue>,
    legacy_message: Option<&str>,
) -> Outcome {
    let english_detail = issue
        .as_ref()
        .and_then(|issue| configuration_detail(Language::English, issue));
    let selected_detail = issue
        .as_ref()
        .and_then(|issue| configuration_detail(language, issue))
        .or_else(|| {
            legacy_message
                .map(|text| language.technical(&text.chars().take(512).collect::<String>()))
        });
    Outcome::Rejected {
        text: with_detail(language, selected_detail.as_deref()),
        message: with_detail(Language::English, english_detail.as_deref()),
    }
}

fn with_detail(language: Language, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => language.message(
            keys::CLI_SETTINGS_REJECTED_WITH_DETAIL,
            &[("detail", detail)],
        ),
        None => language.label(keys::CLI_SETTINGS_REJECTED).to_owned(),
    }
}

fn configuration_detail(language: Language, issue: &ConfigurationIssue) -> Option<String> {
    let key = match issue.reason() {
        Reason::SettingsTooLarge => {
            let bytes = language.technical(&issue.size_limit?.to_string());
            return Some(language.message(keys::CLI_CONFIG_TOO_LARGE, &[("bytes", &bytes)]));
        }
        Reason::SettingsToml => {
            let line = language.technical(&issue.line?.to_string());
            let column = language.technical(&issue.column?.to_string());
            return Some(language.message(
                keys::CLI_CONFIG_INVALID_TOML,
                &[("line", &line), ("column", &column)],
            ));
        }
        Reason::SettingsUnsupportedVersion => {
            let version = language.technical(&issue.schema_version?.to_string());
            return Some(language.message(keys::CLI_CONFIG_VERSION, &[("version", &version)]));
        }
        Reason::SettingsInvalidCpus => keys::CLI_CONFIG_CPUS,
        Reason::SettingsInvalidMemory => keys::CLI_CONFIG_MEMORY,
        Reason::SettingsInvalidIdle => keys::CLI_CONFIG_IDLE,
        Reason::SettingsInvalidDiskLimit => keys::CLI_CONFIG_DISK_LIMIT,
        Reason::SettingsInvalidProxy => keys::CLI_CONFIG_PROXY,
        Reason::SettingsTelemetryUnavailable => keys::CLI_CONFIG_TELEMETRY,
        Reason::SettingsInvalidValue => keys::CLI_CONFIG_INVALID,
        Reason::SettingsStale => return Some(language.label(keys::CLI_CONFIG_STALE).into()),
        Reason::SettingsProxyEnabled => {
            return Some(language.label(keys::CLI_CONFIG_PROXY_ENABLED).into());
        }
        Reason::SettingsEncode => return Some(language.label(keys::CLI_CONFIG_ENCODE).into()),
        Reason::ShareInvalidPath => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_PATH).into());
        }
        Reason::ShareResolutionFailed => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_RESOLVE).into());
        }
        Reason::ShareProtectionUnverified => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_PROTECTION).into());
        }
        Reason::ShareProtected => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_PROTECTED).into());
        }
        Reason::ShareNotDirectory => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_DIRECTORY).into());
        }
        Reason::ShareUnsupportedSource => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_SOURCE).into());
        }
        Reason::ShareNotAllowed => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_NOT_ALLOWED).into());
        }
        Reason::ShareGuestLayout => {
            return Some(language.label(keys::CLI_CONFIG_SHARE_LAYOUT).into());
        }
        Reason::ShareUnknown => return Some(language.label(keys::CLI_CONFIG_SHARE_UNKNOWN).into()),
        Reason::Unspecified => return None,
    };
    let field = language.technical(issue.field.as_deref()?);
    Some(language.message(key, &[("field", &field)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_actionable_typed_details_in_each_language() {
        let issue = ConfigurationIssue {
            reason: Reason::SettingsInvalidCpus.into(),
            field: Some("vm.cpus".into()),
            ..Default::default()
        };
        let Outcome::Rejected { text: english, .. } =
            rejection(Language::English, Some(issue.clone()), Some("old text"))
        else {
            panic!("expected a rejection")
        };
        let Outcome::Rejected {
            text: arabic,
            message,
        } = rejection(Language::Arabic, Some(issue), Some("old text"))
        else {
            panic!("expected a rejection")
        };
        assert_eq!(
            english,
            "The Marsa daemon rejected the request. invalid setting vm.cpus: must be at least one"
        );
        assert!(arabic.contains("\u{2066}vm.cpus\u{2069}"));
        assert!(arabic.contains("على الأقل"));
        assert!(!arabic.contains("old text"));
        assert_eq!(message, english);
    }

    #[test]
    fn a_current_peer_without_recognized_details_uses_local_catalog_copy() {
        let Outcome::Rejected { text, .. } = rejection(Language::Arabic, None, None) else {
            panic!("expected a rejection")
        };
        assert_eq!(text, "رفض خادم مرسى الطلب.");
    }

    #[test]
    fn legacy_arabic_status_is_human_only_and_keeps_json_english() {
        let Outcome::Rejected { text, message } =
            rejection(Language::Arabic, None, Some("رسالة قديمة"))
        else {
            panic!("expected a rejection")
        };
        assert!(text.contains("رسالة قديمة"));
        assert_eq!(message, "The Marsa daemon rejected the request.");
    }

    #[test]
    fn a_legacy_peer_still_gets_a_safe_actionable_diagnostic() {
        let Outcome::Rejected { text, .. } =
            rejection(Language::English, None, Some("bad\nvalue\u{202e}"))
        else {
            panic!("expected a rejection")
        };
        assert_eq!(
            text,
            r"The Marsa daemon rejected the request. bad\nvalue\u{202e}"
        );
    }
}
