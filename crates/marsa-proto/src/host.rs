//! Generated gRPC definitions used by the host daemon and its clients.

pub mod docker_cleanup;

/// The major version of the host API exposed by the generated bindings.
pub const API_MAJOR: u32 = 1;

/// The latest additive revision of the host API.
pub const API_MINOR: u32 = 16;

/// Maximum lookback for a bounded Docker event history request.
pub const DOCKER_ACTIVITY_WINDOW_SECONDS: i64 = 24 * 60 * 60;

/// Build a finite read-only query that includes the current engine second.
pub fn docker_activity_query(now: i64) -> String {
    let until = now.saturating_add(1);
    let since = until.saturating_sub(DOCKER_ACTIVITY_WINDOW_SECONDS);
    format!("/events?since={since}&until={until}")
}

/// Maximum JSON byte length for a live Docker resource collection.
pub const DOCKER_INVENTORY_LIMIT: usize = 8 * 1024 * 1024;

/// gRPC message limit for the Docker JSON and its protobuf envelope.
pub const DOCKER_INVENTORY_RESPONSE_LIMIT: usize = DOCKER_INVENTORY_LIMIT + 128;

/// Filename for delegated agent operations next to the owner daemon socket.
pub use marsa_core::paths::AGENT_SOCKET_NAME;

/// Constrained configuration records exchanged in versioned settings documents.
pub use marsa_core::container_config;

/// Requests, responses, clients and servers for the `marsa.v1` package.
#[allow(missing_docs)]
pub mod v1 {
    tonic::include_proto!("marsa.v1");
}

impl v1::ConfigurationIssue {
    /// Host API revision that introduced these optional status details.
    pub const API_MINOR: u32 = 10;

    /// Report a typed rejection while keeping the message understood by older clients.
    pub fn status(self, code: tonic::Code, legacy_message: impl Into<String>) -> tonic::Status {
        use prost::Message as _;

        tonic::Status::with_details(code, legacy_message, self.encode_to_vec().into())
    }

    /// Read optional configuration details from a settings or shares RPC failure.
    pub fn from_status(status: &tonic::Status) -> Option<Self> {
        use prost::Message as _;

        let bytes = status.details();
        if bytes.is_empty() || bytes.len() > 256 {
            return None;
        }
        let issue = Self::decode(bytes).ok()?;
        (issue.reason() != v1::configuration_issue::Reason::Unspecified).then_some(issue)
    }
}

/// Resolve the typed API's default Unix socket from an absolute user home.
pub fn socket_path(
    home: impl AsRef<std::path::Path>,
) -> Result<std::path::PathBuf, marsa_core::error::PathError> {
    Ok(marsa_core::paths::MarsaPaths::from_home(home)?.daemon_socket())
}

/// Resolve Docker's Unix socket in the same per-user layout as the typed API.
pub fn docker_socket_path(
    home: impl AsRef<std::path::Path>,
) -> Result<std::path::PathBuf, marsa_core::error::PathError> {
    Ok(marsa_core::paths::MarsaPaths::from_home(home)?.docker_socket())
}
