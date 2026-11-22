//! IBM Plex face selection and type roles from the Figma Foundations.

/// A bundled font family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    /// IBM Plex Sans for Latin UI labels.
    Sans,
    /// IBM Plex Sans Arabic for Arabic UI labels and mixed text fallback.
    SansArabic,
    /// IBM Plex Mono for Latin technical values.
    Mono,
}

impl Family {
    /// The canonical family name used by CoreText for face selection.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sans => "IBM Plex Sans",
            Self::SansArabic => "IBM Plex Sans Arabic",
            Self::Mono => "IBM Plex Mono",
        }
    }
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
        let mut font = gpui::font(self.family.name());
        font.weight = match self.weight {
            Weight::Regular => gpui::FontWeight::NORMAL,
            Weight::Medium => gpui::FontWeight::MEDIUM,
            Weight::Semibold => gpui::FontWeight::SEMIBOLD,
        };
        font.fallbacks = Some(gpui::FontFallbacks::from_fonts(vec![
            match self.family {
                Family::SansArabic => Family::Sans,
                Family::Sans | Family::Mono => Family::SansArabic,
            }
            .name()
            .to_owned(),
        ]));
        font
    }

    /// Applies the native font, font size, and line height to an element.
    pub fn apply<E: gpui::Styled>(self, element: E) -> E {
        element
            .font(self.font())
            .text_size(gpui::px(self.size))
            .line_height(gpui::px(self.line_height))
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
