//! Authenticated local gRPC transport for Marsa's CLI, GUI and MCP server.

/// Shared localized text and the generated message keys used by Marsa clients.
pub use marsa_l10n as l10n;

/// Typed host API requests and responses shared by Marsa clients.
pub use marsa_proto::host::v1 as api;

#[cfg(unix)]
mod client;
#[cfg(unix)]
pub mod compose;
#[cfg(unix)]
pub mod docker;
#[cfg(unix)]
mod events;
#[cfg(unix)]
mod platform;
#[cfg(unix)]
pub mod release;
#[cfg(unix)]
pub mod runtime;

#[cfg(unix)]
pub use client::{AgentClient, ClientError, DaemonEndpoint, MarsaClient, docker_socket_path};
#[cfg(unix)]
pub use events::EventSubscription;

#[cfg(all(test, unix))]
mod tests;

/// Generate a version 4 UUID for mutating requests to the owner daemon.
#[cfg(unix)]
pub fn request_id() -> std::io::Result<String> {
    use std::fmt::Write as _;

    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let mut id = String::with_capacity(36);
    for (index, byte) in bytes.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        let _ = write!(id, "{byte:02x}");
    }
    Ok(id)
}

#[cfg(target_os = "macos")]
pub mod git_sources;

/// Login item configuration for the packaged macOS app.
#[cfg(target_os = "macos")]
pub mod login;

#[cfg(all(test, target_os = "macos"))]
mod git_sources_tests;
#[cfg(all(test, unix))]
mod release_tests;
