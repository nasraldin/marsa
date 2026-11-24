//! Forward Docker connections without interpreting HTTP framing or upgraded streams.

/// Bounded parsing of Docker container bind sources shared with the guest authorization policy.
pub mod bind_mounts;

#[cfg(unix)]
mod platform;

#[cfg(unix)]
pub use platform::{Startup, serve};
