//! Shared host daemon services.

#[cfg(unix)]
/// On-demand Linux guest lifecycle and readiness supervision.
pub mod supervisor;

#[cfg(unix)]
pub mod guest_control;
