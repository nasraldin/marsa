//! Limit and validate the separate bind-authorization channel before trusting a query or decision.

use prost::Message;

use super::{
    ProtocolError,
    frame::{bounded_length, encode},
    v1::{
        BindAuthorizationDecision, BindAuthorizationRequest, BindAuthorizationResponse,
        ProtocolVersion,
    },
};

/// Host-initiated vsock stream dedicated to guest bind authorization.
pub const BIND_AUTHORIZATION_PORT: u32 = 5_003;

/// Version of the bind-authorization channel, independent of guest control.
pub const BIND_AUTHORIZATION_VERSION: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };

/// Maximum encoded authorization payload, excluding the frame header.
pub const MAX_BIND_AUTHORIZATION_FRAME_BYTES: usize = 64 * 1024;

/// Maximum number of source paths in one query.
pub const MAX_BIND_AUTHORIZATION_SOURCES: usize = 64;

const MAX_SOURCE_BYTES: usize = 4096;
const MAX_DENIAL_BYTES: usize = 1024;

/// Validate a big-endian authorization frame prefix before allocating a payload buffer.
pub fn bind_authorization_frame_length(header: [u8; 4]) -> Result<usize, ProtocolError> {
    let length = u32::from_be_bytes(header) as usize;
    bounded_length(length, MAX_BIND_AUTHORIZATION_FRAME_BYTES)?;
    Ok(length)
}

/// Decode a complete guest authorization query without the length prefix.
pub fn decode_bind_authorization_request(
    payload: &[u8],
) -> Result<BindAuthorizationRequest, ProtocolError> {
    bounded_length(payload.len(), MAX_BIND_AUTHORIZATION_FRAME_BYTES)?;
    let request = BindAuthorizationRequest::decode(payload).map_err(ProtocolError::Decode)?;
    validate_request(&request)?;
    Ok(request)
}

/// Encode a guest authorization query with its four-byte length prefix.
pub fn encode_bind_authorization_request(
    request: &BindAuthorizationRequest,
) -> Result<Vec<u8>, ProtocolError> {
    validate_request(request)?;
    encode(request, MAX_BIND_AUTHORIZATION_FRAME_BYTES)
}

/// Decode a complete host authorization decision without the length prefix.
pub fn decode_bind_authorization_response(
    payload: &[u8],
) -> Result<BindAuthorizationResponse, ProtocolError> {
    bounded_length(payload.len(), MAX_BIND_AUTHORIZATION_FRAME_BYTES)?;
    let response = BindAuthorizationResponse::decode(payload).map_err(ProtocolError::Decode)?;
    validate_response(&response)?;
    Ok(response)
}

/// Encode a host authorization decision with its four-byte length prefix.
pub fn encode_bind_authorization_response(
    response: &BindAuthorizationResponse,
) -> Result<Vec<u8>, ProtocolError> {
    validate_response(response)?;
    encode(response, MAX_BIND_AUTHORIZATION_FRAME_BYTES)
}

fn validate_header(protocol: Option<ProtocolVersion>, id: u64) -> Result<(), ProtocolError> {
    if protocol != Some(BIND_AUTHORIZATION_VERSION) {
        return Err(ProtocolError::InvalidField("bind_authorization.protocol"));
    }
    if id == 0 {
        return Err(ProtocolError::InvalidField("bind_authorization.request_id"));
    }
    Ok(())
}

fn validate_request(request: &BindAuthorizationRequest) -> Result<(), ProtocolError> {
    validate_header(request.protocol, request.request_id)?;
    if request.sources.is_empty()
        || request.sources.len() > MAX_BIND_AUTHORIZATION_SOURCES
        || request.sources.iter().any(|source| {
            !source.starts_with('/')
                || source.len() > MAX_SOURCE_BYTES
                || source.chars().any(char::is_control)
        })
    {
        return Err(ProtocolError::InvalidField("bind_authorization.sources"));
    }
    Ok(())
}

fn validate_response(response: &BindAuthorizationResponse) -> Result<(), ProtocolError> {
    validate_header(response.protocol, response.request_id)?;
    let valid = match BindAuthorizationDecision::try_from(response.decision) {
        Ok(BindAuthorizationDecision::Allow) => response.message.is_empty(),
        Ok(BindAuthorizationDecision::Deny) => {
            !response.message.is_empty()
                && response.message.len() <= MAX_DENIAL_BYTES
                && !response.message.chars().any(char::is_control)
        }
        _ => false,
    };
    if !valid {
        return Err(ProtocolError::InvalidField("bind_authorization.decision"));
    }
    Ok(())
}
