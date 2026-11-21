//! Bounded protobuf messages and version negotiation for guest control.

mod frame;
mod validation;

pub use frame::{decode_guest, decode_host, encode_guest, encode_host, frame_length};
use thiserror::Error;

use v1::ProtocolVersion;

/// Wire messages in the `marsa.guest.v1` package.
#[allow(missing_docs)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/marsa.guest.v1.rs"));
}

/// Guest control listener port in the VM's vsock namespace.
pub const CONTROL_PORT: u32 = 5_000;

/// Largest accepted length of a serialized guest control envelope, in bytes.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Current guest control version advertised by this build.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };

/// A rejected guest control frame or incompatible protocol version.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// A zero-length frame cannot contain an envelope.
    #[error("the guest control frame is empty")]
    EmptyFrame,
    /// A frame exceeds the protocol's payload limit.
    #[error("guest control frame has {actual} bytes, exceeding the {maximum}-byte limit")]
    FrameTooLarge {
        /// Observed envelope length in bytes.
        actual: usize,
        /// Accepted envelope length for the operation in bytes.
        maximum: usize,
    },
    /// The payload is malformed protobuf.
    #[error("invalid guest control protobuf")]
    Decode(#[source] prost::DecodeError),
    /// A decoded field falls outside its allowed shape.
    #[error("invalid guest control field: {0}")]
    InvalidField(&'static str),
    /// The peer's major version is unsupported.
    #[error("guest control protocol major {peer} is incompatible with local major {local}")]
    IncompatibleMajor {
        /// Major version advertised by the peer.
        peer: u32,
        /// Major version supported by this build.
        local: u32,
    },
}

/// Select the highest minor version supported by both peers in the current major.
pub fn negotiate(peer: ProtocolVersion) -> Result<ProtocolVersion, ProtocolError> {
    if peer.major != PROTOCOL_VERSION.major {
        return Err(ProtocolError::IncompatibleMajor {
            peer: peer.major,
            local: PROTOCOL_VERSION.major,
        });
    }

    Ok(std::cmp::min_by_key(peer, PROTOCOL_VERSION, |version| {
        version.minor
    }))
}
