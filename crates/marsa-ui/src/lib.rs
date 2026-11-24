//! Shared interface components, themes, and localization.

// Keep the architecture's dependency edges until this crate uses them directly.
use marsa_core as _;

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
pub use platform::run;
