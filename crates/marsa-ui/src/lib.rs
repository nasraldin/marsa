//! Shared interface components, themes, and localization.

pub mod tokens;

pub use tokens::{ColorToken, Palette};

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
pub mod direction;

#[cfg(target_os = "macos")]
pub mod locale_format;

#[cfg(target_os = "macos")]
rust_i18n::i18n!("../../locales/components");

#[cfg(target_os = "macos")]
mod assets;

#[cfg(target_os = "macos")]
pub use assets::{brand_mark_for_source, is_brand_mark};

#[cfg(target_os = "macos")]
pub use {
    gpui,
    gpui_base::{DialogClose, FocusTrapElement},
    gpui_component,
    marsa_l10n::{Locale, TextKey, keys as gui_keys},
    platform::{run, show_workspace},
    vt100,
};

/// Typed preferences shared with the daemon.
pub mod preferences {
    pub use marsa_core::settings::{
        AccentColor, DisplaySettings, DownloadFormat, EnvironmentAppearance, EnvironmentIcon,
        Language, LogBufferSize, LogSettings, NetworkMode, NotificationSettings, OperateApproval,
        Settings, TerminalFont, Theme, TimeFormat, UiFont, UiFontSize, UpdateChannel, VmBackend,
        valid_compose_template,
    };
}
