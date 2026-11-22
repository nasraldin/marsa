//! Shared logical sizes and durations for both appearances.

use std::time::Duration;

/// Space between components, measured in logical pixels.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Space {
    /// The smallest gap between closely related items.
    S4 = 4,
    /// A compact inline gap.
    S8 = 8,
    /// A compact control inset.
    S12 = 12,
    /// A standard control or card inset.
    S16 = 16,
    /// The gap between related groups.
    S24 = 24,
    /// Section spacing.
    S32 = 32,
    /// The compact control height.
    S36 = 36,
    /// The standard control height.
    S40 = 40,
    /// The large control height.
    S48 = 48,
    /// The gap between larger content sections.
    S64 = 64,
    /// Space for a large header.
    S72 = 72,
    /// Space between major sections.
    S80 = 80,
}

impl Space {
    /// Convert a spacing token to logical pixels.
    pub const fn pixels(self) -> f32 {
        self as u8 as f32
    }
}

/// How much a corner turns at different component sizes.
#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Radius {
    /// Small indicators and dense elements.
    R4 = 4,
    /// Standard form controls.
    R6 = 6,
    /// Panels and larger controls.
    R8 = 8,
    /// Cards and popovers.
    R12 = 12,
    /// Dialogs and larger panels.
    R16 = 16,
    /// Prominent feature surfaces.
    R24 = 24,
    /// Pills and other fully rounded elements.
    Full = 999,
}

impl Radius {
    /// Convert a corner radius to logical pixels.
    pub const fn pixels(self) -> f32 {
        self as u16 as f32
    }
}

/// Width of a standard border stroke in logical pixels.
pub const STROKE: f32 = 1.;

/// The role of a transition in either appearance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    /// Immediate feedback with no animation.
    Instant,
    /// Small control state changes.
    Fast,
    /// Standard panel transitions.
    Normal,
    /// Larger surface transitions.
    Slow,
}

impl Motion {
    /// Returns zero for decorative transitions when reduced motion is enabled.
    pub const fn duration(self, reduce_motion: bool) -> Duration {
        if reduce_motion {
            return Duration::ZERO;
        }
        Duration::from_millis(match self {
            Self::Instant => 0,
            Self::Fast => 120,
            Self::Normal => 180,
            Self::Slow => 280,
        })
    }
}
