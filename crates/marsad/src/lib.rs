//! Shared host daemon services.

#[cfg(unix)]
/// Agent operation decisions and private project grants.
pub mod agent_policy;

#[cfg(unix)]
/// Negotiation and bounded requests for a Linux guest session.
pub mod guest_control;

#[cfg(unix)]
/// On-demand Linux guest lifecycle and readiness supervision.
pub mod supervisor;

#[cfg(unix)]
/// Local gRPC services and native runtime composition.
pub mod typed_api;
