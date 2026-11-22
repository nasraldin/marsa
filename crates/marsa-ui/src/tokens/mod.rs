//! Semantic design decisions shared by all screens and components.

use marsa_core::settings::Theme;

mod color;
pub mod scale;
pub mod typography;

pub use color::{ColorToken, DIALOG_ELEVATION, Elevation, Rgb};

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
}

impl Palette {
    /// Resolve the stored preference against the current system appearance.
    pub const fn resolve(preference: Theme, appearance: Appearance) -> Self {
        match (preference, appearance) {
            (Theme::Light, _) | (Theme::System, Appearance::Light) => Self::Light,
            (Theme::Dark, _) | (Theme::System, Appearance::Dark) => Self::Dark,
        }
    }
}
