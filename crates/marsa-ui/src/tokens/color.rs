//! Paired sRGB values from the Marsa Figma Foundations palette.

use marsa_core::settings::AccentColor;

use super::Palette;

/// An opaque sRGB design color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb(u32);

impl Rgb {
    /// Red, green and blue bytes in sRGB order.
    pub const fn srgb(self) -> [u8; 3] {
        let [_, red, green, blue] = self.0.to_be_bytes();
        [red, green, blue]
    }
}

#[cfg(target_os = "macos")]
impl From<Rgb> for gpui::Hsla {
    fn from(color: Rgb) -> Self {
        gpui::rgb(color.0).into()
    }
}

/// Foreground, surface, control, and data visualization roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorToken {
    /// `bg/canvas`
    BgCanvas,
    /// `bg/surface`
    BgSurface,
    /// `bg/sidebar`
    BgSidebar,
    /// `bg/elevated`
    BgElevated,
    /// `bg/subtle`
    BgSubtle,
    /// `bg/hover`
    BgHover,
    /// `bg/pressed`
    BgPressed,
    /// `bg/code`
    BgCode,
    /// `glass/rail` in the Figma workspace shell.
    GlassRail,
    /// `glass/control` in the Figma workspace shell.
    GlassControl,
    /// `glass/edge` in the Figma workspace shell.
    GlassEdge,
    /// `text/primary`
    TextPrimary,
    /// `text/muted`
    TextMuted,
    /// `text/on-danger`
    TextOnDanger,
    /// `text/on-accent`
    TextOnAccent,
    /// `border/subtle`
    BorderSubtle,
    /// `border/control`
    BorderControl,
    /// `accent/default`
    AccentDefault,
    /// `accent/hover`
    AccentHover,
    /// `accent/pressed`
    AccentPressed,
    /// `accent/subtle`
    AccentSubtle,
    /// `accent/focus`
    AccentFocus,
    /// `status/running`
    StatusRunning,
    /// `status/stopped`
    StatusStopped,
    /// `status/paused`
    StatusPaused,
    /// `status/error`
    StatusError,
    /// `status/warning`
    StatusWarning,
    /// `status/starting`
    StatusStarting,
    /// `status/running-bg`
    StatusRunningBg,
    /// `status/stopped-bg`
    StatusStoppedBg,
    /// `status/paused-bg`
    StatusPausedBg,
    /// `status/error-bg`
    StatusErrorBg,
    /// `status/warning-bg`
    StatusWarningBg,
    /// `status/starting-bg`
    StatusStartingBg,
    /// `source/api`
    SourceApi,
    /// `source/db`
    SourceDb,
    /// `source/cache`
    SourceCache,
    /// `chart/cpu`
    ChartCpu,
    /// `chart/memory`
    ChartMemory,
    /// `chart/network`
    ChartNetwork,
    /// `chart/disk`
    ChartDisk,
    /// `overlay/scrim`
    OverlayScrim,
    /// `brand/blue`
    BrandBlue,
    /// `brand/orange`
    BrandOrange,
}

/// The dark, bright and tint stops of the Figma accent swatch.
pub const fn accent_swatch(color: AccentColor) -> [Rgb; 3] {
    let (dark, bright, tint) = match color {
        AccentColor::MarsaBlue => (0x085aa6, 0x7cc4ff, 0xdceeff),
        AccentColor::Ocean => (0x00576c, 0x73deee, 0xddf3fa),
        AccentColor::Fern => (0x2a5925, 0xafe287, 0xe8f4df),
        AccentColor::Coral => (0x903246, 0xffaca6, 0xfce9e8),
        AccentColor::Amber => (0x754604, 0xf9d58a, 0xfff0d5),
        AccentColor::Violet => (0x693589, 0xdbb7ff, 0xf2e8fc),
    };
    [Rgb(dark), Rgb(bright), Rgb(tint)]
}

const fn mix_rgb(base: u32, tint: u32, percent: u32) -> Rgb {
    let red = blend_channel(base >> 16 & 255, tint >> 16 & 255, percent);
    let green = blend_channel(base >> 8 & 255, tint >> 8 & 255, percent);
    let blue = blend_channel(base & 255, tint & 255, percent);
    Rgb(red << 16 | green << 8 | blue)
}

const fn blend_channel(base: u32, tint: u32, percent: u32) -> u32 {
    (base * (100 - percent) + tint * percent) / 100
}

impl Palette {
    /// Returns the sRGB color for a role in the active palette.
    pub const fn color(self, role: ColorToken) -> Rgb {
        if let Self::Custom(_, accent) = self {
            let [deep, bright, tint] = accent_swatch(accent);
            let dark = self.is_dark();
            match role {
                ColorToken::AccentDefault | ColorToken::AccentFocus => {
                    return if dark { bright } else { deep };
                }
                ColorToken::AccentHover => {
                    return if dark {
                        mix_rgb(bright.0, 0xffffff, 15)
                    } else {
                        mix_rgb(deep.0, 0, 10)
                    };
                }
                ColorToken::AccentPressed => {
                    return if dark {
                        mix_rgb(bright.0, deep.0, 15)
                    } else {
                        mix_rgb(deep.0, 0, 20)
                    };
                }
                ColorToken::AccentSubtle => {
                    return if dark {
                        mix_rgb(0x192337, bright.0, 20)
                    } else {
                        tint
                    };
                }
                ColorToken::GlassRail => {
                    return if dark {
                        mix_rgb(0x192337, bright.0, 11)
                    } else {
                        mix_rgb(0xfbfdff, tint.0, 65)
                    };
                }
                ColorToken::GlassControl => {
                    return if dark {
                        mix_rgb(0x192337, bright.0, 16)
                    } else {
                        mix_rgb(0xfbfdff, tint.0, 40)
                    };
                }
                ColorToken::GlassEdge => {
                    return if dark {
                        mix_rgb(0x192337, bright.0, 42)
                    } else {
                        mix_rgb(0xd5e1f0, deep.0, 22)
                    };
                }
                _ => {}
            }
        }
        let (light, dark) = match role {
            ColorToken::BgCanvas => (0xe2eaf6, 0x0b1220),
            ColorToken::BgSurface => (0xf4f7fc, 0x101827),
            ColorToken::BgSidebar => (0xedf3fc, 0x192337),
            ColorToken::BgElevated => (0xfbfdff, 0x1b273a),
            ColorToken::BgSubtle => (0xe7eff9, 0x26354a),
            ColorToken::BgHover => (0xe7ebf4, 0x282f42),
            ColorToken::BgPressed => (0xe0e6f3, 0x30394b),
            ColorToken::BgCode => (0xedf3fa, 0x111a29),
            ColorToken::GlassRail => (0xf1f8ff, 0x1c2d45),
            ColorToken::GlassControl => (0xf6fbff, 0x23364f),
            ColorToken::GlassEdge => (0xb3cce8, 0x4c6c8e),
            ColorToken::TextPrimary => (0x171c2b, 0xeff2fa),
            ColorToken::TextMuted => (0x4c5870, 0xbac3d6),
            ColorToken::TextOnDanger => (0xffffff, 0x230b12),
            ColorToken::TextOnAccent => (0xffffff, 0x06203a),
            ColorToken::BorderSubtle => (0xd5e1f0, 0x35455f),
            ColorToken::BorderControl => (0x758198, 0x73829e),
            ColorToken::AccentDefault => (0x0a66ba, 0x7cc4ff),
            ColorToken::AccentHover => (0x085aa6, 0xa3d5ff),
            ColorToken::AccentPressed => (0x064e92, 0x5faef2),
            ColorToken::AccentSubtle => (0xdceeff, 0x1b3a5c),
            ColorToken::AccentFocus => (0x0c73d0, 0x8ccbff),
            ColorToken::StatusRunning => (0x176344, 0x8ccfb1),
            ColorToken::StatusStopped => (0x525d74, 0xbdc6da),
            ColorToken::StatusPaused => (0x794c0f, 0xf2c07e),
            ColorToken::StatusError => (0x9f2947, 0xffb0c2),
            ColorToken::StatusWarning => (0x794c0f, 0xf2c07e),
            ColorToken::StatusStarting => (0x0b5fb0, 0x9cceff),
            ColorToken::StatusRunningBg => (0xe8f3ed, 0x223830),
            ColorToken::StatusStoppedBg => (0xebeef5, 0x2c3446),
            ColorToken::StatusPausedBg => (0xfaeedd, 0x412f25),
            ColorToken::StatusErrorBg => (0xfbeaf0, 0x412637),
            ColorToken::StatusWarningBg => (0xfaeedd, 0x412f25),
            ColorToken::StatusStartingBg => (0xe4f0fd, 0x1f3550),
            ColorToken::SourceApi => (0x0b5fb0, 0x9cceff),
            ColorToken::SourceDb => (0x5d4095, 0xd0bdf8),
            ColorToken::SourceCache => (0x89460e, 0xffbe90),
            ColorToken::ChartCpu => (0x0a66ba, 0x7cc4ff),
            ColorToken::ChartMemory => (0x62449c, 0xc6b3f3),
            ColorToken::ChartNetwork => (0x146769, 0x80cdc8),
            ColorToken::ChartDisk => (0x89460e, 0xefb480),
            ColorToken::OverlayScrim => (0x172039, 0x070a11),
            ColorToken::BrandBlue => (0x088dec, 0x088dec),
            ColorToken::BrandOrange => (0xff8a1f, 0xff8a1f),
        };
        Rgb(if self.is_dark() { dark } else { light })
    }
}

/// The Figma dialog shadow used on elevated panels in both appearances.
pub const DIALOG_ELEVATION: Elevation = Elevation {
    offset_y: 12.,
    blur: 32.,
};

/// The geometry of a shadow cast below a raised surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elevation {
    /// Downward offset in logical pixels.
    pub offset_y: f32,
    /// Blur radius in logical pixels.
    pub blur: f32,
}

#[cfg(target_os = "macos")]
impl Elevation {
    /// Builds the shadow for a GPUI element.
    pub fn shadow(self) -> gpui::BoxShadow {
        let mut shadow = gpui::BoxShadow::new(
            gpui::px(0.),
            gpui::px(self.offset_y),
            gpui::rgba(0x03121740).into(),
        );
        shadow.blur_radius = gpui::px(self.blur);
        shadow
    }
}
