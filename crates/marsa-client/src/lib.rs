//! Authenticated local gRPC transport for Marsa's CLI, GUI and MCP server.

/// Shared localized text and the generated message keys used by Marsa clients.
pub use marsa_l10n as l10n;

/// Typed host API requests and responses shared by Marsa clients.
pub use marsa_proto::host::v1 as api;

#[cfg(unix)]
mod client;
#[cfg(unix)]
mod events;
#[cfg(unix)]
mod platform;

#[cfg(unix)]
pub use client::{ClientError, DaemonEndpoint, MarsaClient, docker_socket_path};
#[cfg(unix)]
pub use events::EventSubscription;

#[cfg(all(test, unix))]
mod tests;
