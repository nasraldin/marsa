//! Generated gRPC definitions used by the host daemon and its clients.

/// The major version of the host API exposed by the generated bindings.
pub const API_MAJOR: u32 = 1;

/// The latest additive revision of the host API.
pub const API_MINOR: u32 = 5;

/// Requests, responses, clients and servers for the `marsa.v1` package.
#[allow(missing_docs)]
pub mod v1 {
    tonic::include_proto!("marsa.v1");
}

/// Resolve the typed API's default Unix socket from an absolute user home.
pub fn socket_path(
    home: impl AsRef<std::path::Path>,
) -> Result<std::path::PathBuf, marsa_core::error::PathError> {
    Ok(marsa_core::paths::MarsaPaths::from_home(home)?.daemon_socket())
}
