//! Interface and terminal face selection and type roles from the Figma Foundations.

use marsa_core::settings::{TerminalFont, UiFont, UiFontSize};

const DEFAULT_REM: f32 = 16.;

/// A bundled or system-installed font family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    /// IBM Plex Sans for Latin UI labels.
    Sans,
    /// IBM Plex Sans Arabic for Arabic UI labels and mixed text fallback.
    SansArabic,
    /// IBM Plex Mono for Latin technical values.
    Mono,
    /// The native operating system interface face.
    System,
    /// The operating system's Menlo monospace face.
    Menlo,
}

impl Family {
    /// The canonical family name used by CoreText for face selection.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sans => "IBM Plex Sans",
            Self::SansArabic => "IBM Plex Sans Arabic",
            Self::Mono => "IBM Plex Mono",
            Self::System => ".SystemUIFont",
            Self::Menlo => "Menlo",
        }
    }

    /// Resolve the terminal's monospace font family.
    pub const fn terminal(preference: TerminalFont) -> Self {
        match preference {
            TerminalFont::IbmPlexMono => Self::Mono,
            TerminalFont::Menlo => Self::Menlo,
        }
    }

    #[cfg(target_os = "macos")]
    /// Configure a native face and retain the bundled fallback for mixed-script text.
    pub fn font(self) -> gpui::Font {
        let mut font = gpui::font(self.name());
        font.fallbacks = Some(gpui::FontFallbacks::from_fonts(vec![
            match self {
                Self::SansArabic => Self::Sans,
                Self::Sans | Self::Mono | Self::System | Self::Menlo => Self::SansArabic,
            }
            .name()
            .to_owned(),
        ]));
        font
    }
}

/// GPUI rem size that scales with the selected 12, 14, or 16 pixel body style.
pub const fn rem_size(preference: UiFontSize) -> f32 {
    let body = match preference {
        UiFontSize::Small => 12.,
        UiFontSize::Default => 14.,
        UiFontSize::Large => 16.,
    };
    DEFAULT_REM * (body / 14.)
}

/// Convert nominal interface text pixels into a length that follows the UI font size.
#[cfg(target_os = "macos")]
pub const fn ui_size(pixels: f32) -> gpui::Rems {
    gpui::rems(pixels / DEFAULT_REM)
}

/// The font weights present in the bundled design assets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Weight {
    /// Body and technical text.
    Regular,
    /// Descriptive labels.
    Medium,
    /// Headings and prominent labels.
    Semibold,
}

/// The face and layout metrics for one type role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    /// Primary font family.
    pub family: Family,
    /// Font weight within the family.
    pub weight: Weight,
    /// Font size in logical pixels.
    pub size: f32,
    /// Line height in logical pixels.
    pub line_height: f32,
    /// Tracking from the Foundations design in logical pixels.
    pub tracking: f32,
}

#[cfg(target_os = "macos")]
impl TypeStyle {
    /// Configures native face, weight and script fallback for this text role.
    pub fn font(self) -> gpui::Font {
        let mut font = self.family.font();
        font.weight = match self.weight {
            Weight::Regular => gpui::FontWeight::NORMAL,
            Weight::Medium => gpui::FontWeight::MEDIUM,
            Weight::Semibold => gpui::FontWeight::SEMIBOLD,
        };
        font
    }

    /// Applies the native font, font size, and line height to an element.
    pub fn apply<E: gpui::Styled>(self, element: E) -> E {
        element
            .font(self.font())
            .text_size(ui_size(self.size))
            .line_height(ui_size(self.line_height))
    }
}

/// Typography roles shared by the light and dark themes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// The most prominent screen heading.
    Hero,
    /// The regular screen heading.
    Title,
    /// A heading over a group of content.
    Section,
    /// A short introductory line.
    Lead,
    /// General interface text.
    Body,
    /// Control and navigation labels.
    Label,
    /// Compact supporting text.
    Small,
    /// Very compact supporting labels.
    Caption,
    /// Paths, identifiers, and other technical values.
    Mono,
    /// Compact technical values.
    MonoSmall,
}

impl Role {
    /// Resolve a locale's type role with the selected interface font.
    pub const fn with_ui_font(self, arabic: bool, preference: UiFont) -> TypeStyle {
        let mut style = self.style(arabic);
        if matches!(preference, UiFont::System) && !matches!(self, Self::Mono | Self::MonoSmall) {
            style.family = Family::System;
        }
        style
    }

    /// Resolve a text role for English UI text.
    pub const fn latin(self) -> TypeStyle {
        self.style(false)
    }

    /// Resolve a text role for Arabic UI text.
    pub const fn arabic(self) -> TypeStyle {
        self.style(true)
    }

    const fn style(self, arabic: bool) -> TypeStyle {
        use Weight::{Medium, Regular, Semibold};

        let (weight, size, latin_line, arabic_line, latin_tracking) = match self {
            Self::Hero => (Semibold, 40., 48., 52., -0.65),
            Self::Title => (Semibold, 28., 36., 40., -0.4),
            Self::Section => (Semibold, 20., 28., 32., 0.),
            Self::Lead => (Medium, 16., 24., 28., 0.),
            Self::Body => (Regular, 14., 20., 24., 0.),
            Self::Label => (Medium, 13., 20., 24., 0.),
            Self::Small => (Regular, 12., 16., 20., 0.),
            Self::Caption => (Medium, 11., 16., 20., 0.),
            Self::Mono => (Regular, 12., 20., 20., 0.),
            Self::MonoSmall => (Regular, 11., 16., 16., 0.),
        };
        let family = match (self, arabic) {
            (Self::Mono | Self::MonoSmall, _) => Family::Mono,
            (_, true) => Family::SansArabic,
            (_, false) => Family::Sans,
        };
        TypeStyle {
            family,
            weight,
            size,
            line_height: if arabic { arabic_line } else { latin_line },
            tracking: if arabic { 0. } else { latin_tracking },
        }
    }
}
