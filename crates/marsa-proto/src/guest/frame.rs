//! Enforce wire limits before allocating or decoding guest frames.

use prost::Message;

use super::v1::{GuestEnvelope, HostEnvelope};
use super::{MAX_FRAME_BYTES, ProtocolError, validation};

const HEADER_BYTES: usize = 4;

/// Validate the big-endian payload length before reserving memory for a frame.
pub fn frame_length(header: [u8; HEADER_BYTES]) -> Result<usize, ProtocolError> {
    let length = u32::from_be_bytes(header) as usize;
    bounded_length(length, MAX_FRAME_BYTES)?;
    Ok(length)
}

/// Decode and validate a complete host protobuf payload, excluding its length prefix.
pub fn decode_host(payload: &[u8]) -> Result<HostEnvelope, ProtocolError> {
    let envelope = decode(payload)?;
    bounded_length(payload.len(), validation::host(&envelope)?)?;
    Ok(envelope)
}

/// Decode and validate a complete guest protobuf payload, excluding its length prefix.
pub fn decode_guest(payload: &[u8]) -> Result<GuestEnvelope, ProtocolError> {
    let envelope = decode(payload)?;
    bounded_length(payload.len(), validation::guest(&envelope)?)?;
    Ok(envelope)
}

/// Validate and encode a host envelope with its four-byte big-endian length prefix.
pub fn encode_host(envelope: &HostEnvelope) -> Result<Vec<u8>, ProtocolError> {
    encode(envelope, validation::host(envelope)?)
}

/// Validate and encode a guest envelope with its four-byte big-endian length prefix.
pub fn encode_guest(envelope: &GuestEnvelope) -> Result<Vec<u8>, ProtocolError> {
    encode(envelope, validation::guest(envelope)?)
}

fn bounded_length(actual: usize, maximum: usize) -> Result<(), ProtocolError> {
    if actual == 0 {
        return Err(ProtocolError::EmptyFrame);
    }
    if actual > maximum {
        return Err(ProtocolError::FrameTooLarge { actual, maximum });
    }
    Ok(())
}

fn decode<M: Message + Default>(payload: &[u8]) -> Result<M, ProtocolError> {
    bounded_length(payload.len(), MAX_FRAME_BYTES)?;
    M::decode(payload).map_err(ProtocolError::Decode)
}

fn encode(envelope: &impl Message, maximum: usize) -> Result<Vec<u8>, ProtocolError> {
    let length = envelope.encoded_len();
    bounded_length(length, maximum)?;

    let mut frame = Vec::with_capacity(HEADER_BYTES + length);
    // The validated length is at most MAX_FRAME_BYTES, which fits in u32.
    frame.extend_from_slice(&(length as u32).to_be_bytes());
    envelope.encode_raw(&mut frame);
    Ok(frame)
}
