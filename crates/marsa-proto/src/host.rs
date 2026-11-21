//! Generated gRPC definitions used by the host daemon and its clients.

/// The major version of the host API exposed by the generated bindings.
pub const API_MAJOR: u32 = 1;

/// The latest additive revision of the host API.
pub const API_MINOR: u32 = 0;

/// Requests, responses, clients and servers for the `marsa.v1` package.
#[allow(missing_docs)]
pub mod v1 {
    tonic::include_proto!("marsa.v1");
}
