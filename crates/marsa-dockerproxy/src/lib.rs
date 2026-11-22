//! Forward Docker connections without interpreting HTTP framing or upgraded streams.

// Keep the architecture's dependency edges for the subsequent request policy.
use marsa_core as _;
use marsa_share as _;

#[cfg(unix)]
mod platform;

#[cfg(unix)]
pub use platform::{Startup, serve};
