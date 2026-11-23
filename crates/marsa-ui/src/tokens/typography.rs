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
    /// The native family name of a bundled or installed optional typeface.
    Named(&'static str),
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
            Self::Named(name) => name,
        }
    }

    /// Resolve the user's font preference for interface text.
    pub const fn interface(preference: UiFont, arabic: bool) -> Self {
        match preference {
            UiFont::IbmPlex if arabic => Self::SansArabic,
            UiFont::IbmPlex => Self::Sans,
            UiFont::System => Self::System,
            UiFont::Geist => Self::Named("Geist"),
            UiFont::Inter => Self::Named("Inter"),
            UiFont::PlusJakartaSans => Self::Named("Plus Jakarta Sans"),
            UiFont::DmSans => Self::Named("DM Sans"),
            UiFont::Outfit => Self::Named("Outfit"),
            UiFont::SpaceGrotesk => Self::Named("Space Grotesk"),
            UiFont::SofiaSans => Self::Named("Sofia Sans"),
            UiFont::Nunito => Self::Named("Nunito"),
            UiFont::Poppins => Self::Named("Poppins"),
            UiFont::Montserrat => Self::Named("Montserrat"),
            UiFont::Raleway => Self::Named("Raleway"),
            UiFont::Manrope => Self::Named("Manrope"),
            UiFont::Roboto => Self::Named("Roboto"),
            UiFont::OpenSans => Self::Named("Open Sans"),
            UiFont::Lato => Self::Named("Lato"),
            UiFont::SourceSans3 => Self::Named("Source Sans 3"),
            UiFont::WorkSans => Self::Named("Work Sans"),
            UiFont::FiraSans => Self::Named("Fira Sans"),
            UiFont::JetBrainsMono => Self::Named("JetBrains Mono"),
            UiFont::FiraCode => Self::Named("Fira Code"),
            UiFont::Quicksand => Self::Named("Quicksand"),
            UiFont::Comfortaa => Self::Named("Comfortaa"),
        }
    }

    /// Find the terminal family requested by a saved preference.
    pub const fn terminal(preference: TerminalFont) -> Self {
        match preference {
            TerminalFont::IbmPlexMono => Self::Mono,
            TerminalFont::Menlo => Self::Menlo,
            TerminalFont::System => Self::Named(".AppleSystemUIFontMonospaced"),
            TerminalFont::JetBrainsMono => Self::Named("JetBrains Mono"),
            TerminalFont::FiraCode => Self::Named("Fira Code"),
            TerminalFont::SourceCodePro => Self::Named("Source Code Pro"),
            TerminalFont::CascadiaCode => Self::Named("Cascadia Code"),
            TerminalFont::RobotoMono => Self::Named("Roboto Mono"),
            TerminalFont::UbuntuMono => Self::Named("Ubuntu Mono"),
            TerminalFont::SpaceMono => Self::Named("Space Mono"),
            TerminalFont::Inconsolata => Self::Named("Inconsolata"),
            TerminalFont::AnonymousPro => Self::Named("Anonymous Pro"),
            TerminalFont::DmMono => Self::Named("DM Mono"),
            TerminalFont::RedHatMono => Self::Named("Red Hat Mono"),
            TerminalFont::Consolas => Self::Named("Consolas"),
            TerminalFont::SfMono => Self::Named("SF Mono"),
        }
    }

    /// Return whether the optional family is provided by the app or the system.
    #[cfg(target_os = "macos")]
    pub fn available(self, cx: &gpui::App) -> bool {
        match self {
            Self::Named(name @ ("Consolas" | "SF Mono")) => cx
                .try_global::<crate::assets::InstalledMonoFonts>()
                .is_some_and(|fonts| fonts.contains(name)),
            _ => true,
        }
    }

    /// Keep the terminal monospace if a saved system font becomes unavailable.
    #[cfg(target_os = "macos")]
    pub fn available_terminal(preference: TerminalFont, cx: &gpui::App) -> Self {
        let family = Self::terminal(preference);
        if family.available(cx) {
            family
        } else {
            Self::Mono
        }
    }

    #[cfg(target_os = "macos")]
    /// Configure a native face and retain the bundled fallback for mixed-script text.
    pub fn font(self) -> gpui::Font {
        let mut font = gpui::font(self.name());
        font.fallbacks = Some(gpui::FontFallbacks::from_fonts(vec![
            match self {
                Self::SansArabic => Self::Sans,
                Self::Sans | Self::Mono | Self::System | Self::Menlo | Self::Named(_) => {
                    Self::SansArabic
                }
            }
            .name()
            .to_owned(),
        ]));
        font
    }
}

/// GPUI rem size that scales with the selected body text preference.
pub const fn rem_size(preference: UiFontSize) -> f32 {
    let body = match preference {
        UiFontSize::ExtraSmall => 11.,
        UiFontSize::Small => 12.,
        UiFontSize::Default => 14.,
        UiFontSize::Medium => 15.,
        UiFontSize::Large => 16.,
        UiFontSize::ExtraLarge => 18.,
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
        if !matches!(self, Self::Mono | Self::MonoSmall) {
            style.family = Family::interface(preference, arabic);
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
