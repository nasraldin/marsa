//! UI, update, agent, and privacy preferences shared across host clients.

use serde::{Deserialize, Serialize};

/// Display language for user-facing text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// Follow the operating system's language.
    #[default]
    #[serde(rename = "system")]
    System,
    /// Display American English.
    #[serde(rename = "en-US")]
    English,
    /// Display Modern Standard Arabic.
    #[serde(rename = "ar")]
    Arabic,
}

/// Color scheme preference.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating system's appearance.
    #[default]
    System,
    /// Use the light color scheme.
    Light,
    /// Use the dark color scheme.
    Dark,
}

/// Desktop app appearance and language settings.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// Locale preference, separate from the layout direction inferred from it.
    pub language: Language,
    /// Appearance preference applied on all screens.
    pub theme: Theme,
}

/// Release stream the updater follows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    /// Releases that have been promoted to stable.
    #[default]
    Stable,
    /// Opt-in beta releases.
    Beta,
}

/// Update stream and automation preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateSettings {
    /// Release stream to check.
    pub channel: UpdateChannel,
    /// Check for updates automatically.
    pub auto_check: bool,
    /// Install downloaded updates automatically when permitted.
    pub auto_install: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::default(),
            auto_check: true,
            auto_install: false,
        }
    }
}

/// How frequently the agent policy engine asks for an operate-tier approval.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperateApproval {
    /// Ask once per client and project, subject to approval and grant expiration.
    #[default]
    PerProject,
    /// Require approval for each operation.
    AlwaysAsk,
    /// Refuse all operate-tier requests.
    Deny,
}

/// Default permissions that agents receive before any grant.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSettings {
    /// Approval policy for operate-tier actions.
    pub operate_approval: OperateApproval,
    /// Permit asking for advanced operations, which still require approval each time.
    pub advanced_access: bool,
}

/// Privacy settings supported by the current build.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetrySettings {
    /// Reserved opt-in. This build accepts only `false`.
    pub enabled: bool,
}
