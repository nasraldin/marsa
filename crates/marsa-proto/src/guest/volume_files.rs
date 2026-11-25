//! Independently bounded wire frames for the host-only volume read service.

use prost::Message as _;

use super::{
    ProtocolError, frame,
    v1::{
        SelectVolumeRead, VolumeReadFailure, VolumeReadFormat, VolumeReadReply, VolumeReadRequest,
        volume_read_reply::Body as Reply, volume_read_request::Body as Request,
    },
};

/// Host-only guest listener for confined volume reads.
pub const VOLUME_FILES_PORT: u32 = 5_004;
/// Largest byte chunk in either relayed stream.
pub const VOLUME_READ_CHUNK: usize = 64 * 1024;
/// Maximum total size of a raw file or generated TAR.
pub const VOLUME_READ_LIMIT: u64 = 16 * 1024 * 1024 * 1024;

const FRAME_LIMIT: usize = VOLUME_READ_CHUNK + 16;
const PATH_LIMIT: usize = 4096;

/// Check the frame length before the reader allocates its payload buffer.
pub fn volume_frame_length(header: [u8; 4]) -> Result<usize, ProtocolError> {
    let length = u32::from_be_bytes(header) as usize;
    frame::bounded_length(length, FRAME_LIMIT)?;
    Ok(length)
}

/// Validate the Docker volume name and creation time before querying the engine.
pub fn validate_volume_selection(request: &SelectVolumeRead) -> Result<(), ProtocolError> {
    let name = request.name.as_bytes();
    if name.len() > 255
        || !name.first().is_some_and(u8::is_ascii_alphanumeric)
        || !name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(byte))
    {
        return Err(ProtocolError::InvalidField("volume name"));
    }
    if request.created_at.is_empty()
        || request.created_at.len() > 64
        || !request
            .created_at
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err(ProtocolError::InvalidField("volume creation time"));
    }
    Ok(())
}

/// Validate an absolute path in the pinned volume.
pub fn validate_volume_path(path: &str) -> Result<(), ProtocolError> {
    if path.len() > PATH_LIMIT
        || path.chars().any(char::is_control)
        || !path.starts_with('/')
        || path != "/"
            && path[1..]
                .split('/')
                .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(ProtocolError::InvalidField("volume path"));
    }
    Ok(())
}

fn validate_request(request: &VolumeReadRequest) -> Result<(), ProtocolError> {
    match &request.body {
        Some(Request::Probe(true) | Request::Start(true)) => Ok(()),
        Some(Request::Select(request)) => validate_volume_selection(request),
        Some(Request::Prepare(request)) => validate_volume_path(&request.path),
        _ => Err(ProtocolError::InvalidField("volume request")),
    }
}

fn validate_reply(reply: &VolumeReadReply) -> Result<(), ProtocolError> {
    match &reply.body {
        Some(Reply::Capable(true) | Reply::Selected(true) | Reply::Complete(true)) => Ok(()),
        Some(Reply::Format(value))
            if matches!(
                VolumeReadFormat::try_from(*value),
                Ok(VolumeReadFormat::Tar | VolumeReadFormat::SingleFile)
            ) =>
        {
            Ok(())
        }
        Some(Reply::Failure(value))
            if VolumeReadFailure::try_from(*value)
                .is_ok_and(|failure| failure != VolumeReadFailure::Unspecified) =>
        {
            Ok(())
        }
        Some(Reply::Data(bytes)) => frame::bounded_length(bytes.len(), VOLUME_READ_CHUNK),
        _ => Err(ProtocolError::InvalidField("volume reply")),
    }
}

/// Decode a validated request payload.
pub fn decode_volume_request(payload: &[u8]) -> Result<VolumeReadRequest, ProtocolError> {
    frame::bounded_length(payload.len(), FRAME_LIMIT)?;
    let request = VolumeReadRequest::decode(payload).map_err(ProtocolError::Decode)?;
    validate_request(&request)?;
    Ok(request)
}

/// Encode a request with its bounded frame prefix.
pub fn encode_volume_request(request: &VolumeReadRequest) -> Result<Vec<u8>, ProtocolError> {
    validate_request(request)?;
    frame::encode(request, FRAME_LIMIT)
}

/// Decode a validated response payload.
pub fn decode_volume_reply(payload: &[u8]) -> Result<VolumeReadReply, ProtocolError> {
    frame::bounded_length(payload.len(), FRAME_LIMIT)?;
    let reply = VolumeReadReply::decode(payload).map_err(ProtocolError::Decode)?;
    validate_reply(&reply)?;
    Ok(reply)
}

/// Encode a response with its bounded frame prefix.
pub fn encode_volume_reply(reply: &VolumeReadReply) -> Result<Vec<u8>, ProtocolError> {
    validate_reply(reply)?;
    frame::encode(reply, FRAME_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_both_relays_and_rejects_ambiguous_paths() {
        let selection = SelectVolumeRead {
            name: "fixture-data".into(),
            created_at: "2026-11-26T00:00:00Z".into(),
        };
        let request = VolumeReadRequest {
            body: Some(Request::Select(selection)),
        };
        let encoded = encode_volume_request(&request).unwrap();
        assert_eq!(decode_volume_request(&encoded[4..]).unwrap(), request);
        for path in [
            "file",
            "/../data",
            "//data",
            "/data/",
            "/data/./file",
            "/\0",
        ] {
            assert!(validate_volume_path(path).is_err(), "{path}");
        }
        let reply = VolumeReadReply {
            body: Some(Reply::Data(vec![0; VOLUME_READ_CHUNK + 1])),
        };
        assert!(encode_volume_reply(&reply).is_err());
        assert!(volume_frame_length((u32::MAX).to_be_bytes()).is_err());
    }
}
