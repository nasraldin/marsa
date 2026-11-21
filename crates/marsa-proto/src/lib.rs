//! Wire formats for the host API and the guest control channel.

// Keep the architecture's dependency edge until this crate can use core types directly.
use marsa_core as _;

pub mod guest;

#[cfg(feature = "host-api")]
pub mod host;
