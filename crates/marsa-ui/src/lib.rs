//! Shared interface components, themes, and localization.

pub mod tokens;

pub use tokens::{ColorToken, Palette};

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
pub mod direction;

#[cfg(target_os = "macos")]
pub use {
    gpui, gpui_component,
    marsa_l10n::{Locale, TextKey, keys as gui_keys},
    platform::run,
};

/// Typed preferences shared with the daemon.
pub mod preferences {
    pub use marsa_core::settings::{
        Language, NetworkMode, OperateApproval, Settings, Theme, UpdateChannel, VmBackend,
    };
}
