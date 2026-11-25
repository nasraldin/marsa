//! Semantic design decisions shared by all screens and components.

use marsa_core::settings::{AccentColor, Theme};

mod color;
#[cfg(target_os = "macos")]
mod components;
pub mod scale;
#[cfg(test)]
mod tests;
pub mod typography;

pub use color::{ColorToken, DIALOG_ELEVATION, Elevation, Rgb, accent_swatch};

/// The appearance supplied by the operating system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Appearance {
    /// The operating system is using its light appearance.
    Light,
    /// The operating system is using its dark appearance.
    Dark,
}

/// The palette selected from the current preference and appearance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Palette {
    /// The light semantic color values.
    Light,
    /// The dark semantic color values.
    Dark,
    /// The current appearance with a chosen accent.
    Custom(Appearance, AccentColor),
}

impl Palette {
    /// Resolve the stored preference against the current system appearance.
    pub const fn resolve(preference: Theme, appearance: Appearance) -> Self {
        match (preference, appearance) {
            (Theme::Light, _) | (Theme::System, Appearance::Light) => Self::Light,
            (Theme::Dark, _) | (Theme::System, Appearance::Dark) => Self::Dark,
        }
    }

    /// Replace the accent without changing the active appearance.
    pub const fn with_accent(self, accent: AccentColor) -> Self {
        if matches!(accent, AccentColor::MarsaBlue) {
            return if self.is_dark() {
                Self::Dark
            } else {
                Self::Light
            };
        }
        let appearance = if self.is_dark() {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        Self::Custom(appearance, accent)
    }

    /// Return whether this palette uses the dark scheme.
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark | Self::Custom(Appearance::Dark, _))
    }
}
