//! Shared host daemon services.

#[cfg(unix)]
/// On-demand Linux guest lifecycle and readiness supervision.
pub mod supervisor;

#[cfg(unix)]
/// Local gRPC services and native runtime composition.
pub mod typed_api;
