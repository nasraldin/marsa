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

/// Accent choices shown in the desktop's General settings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentColor {
    /// The default Marsa blue.
    #[default]
    MarsaBlue,
    /// Cool cyan accent.
    Ocean,
    /// Green accent.
    Fern,
    /// Warm rose accent.
    Coral,
    /// Amber accent.
    Amber,
    /// Purple accent.
    Violet,
}

impl AccentColor {
    fn is_default(&self) -> bool {
        *self == Self::MarsaBlue
    }
}

/// Maximum UTF-8 byte length of a saved Compose template.
pub const MAX_COMPOSE_TEMPLATE_BYTES: usize = 16 * 1024;

/// Check that a Compose draft fits in desktop settings and does not contain NUL.
pub fn valid_compose_template(value: &str) -> bool {
    value.len() <= MAX_COMPOSE_TEMPLATE_BYTES && !value.trim().is_empty() && !value.contains('\0')
}

/// Desktop app appearance, language and workflow settings.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// Locale preference, separate from the layout direction inferred from it.
    pub language: Language,
    /// Appearance preference applied on all screens.
    pub theme: Theme,
    /// Tint used by interactive desktop controls.
    #[serde(skip_serializing_if = "AccentColor::is_default")]
    pub accent_color: AccentColor,
    /// Whether the owner completed the desktop setup flow.
    #[serde(skip_serializing_if = "is_false")]
    pub setup_complete: bool,
    /// macOS notification preferences for each event category.
    pub notifications: NotificationSettings,
    /// User-authored Compose YAML used when starting a new stack.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_template: Option<String>,
}

/// Desktop notifications are opt-in and independent of in-window state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationSettings {
    /// Notify after the engine enters a failed state.
    pub engine_failure: bool,
    /// Notify when a verified build is announced by the updater.
    pub update_available: bool,
    /// Notify when a new request needs the owner's approval.
    pub approval_pending: bool,
    /// Notify when the VM changes between native and userspace networking.
    pub network_mode_changed: bool,
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

fn is_false(value: &bool) -> bool {
    !*value
}
