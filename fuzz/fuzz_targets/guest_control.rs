//! Check framing limits and lossless canonical encoding for both control directions.

#![no_main]

use libfuzzer_sys::fuzz_target;
use marsa_proto::guest::{
    MAX_FRAME_BYTES, decode_guest, decode_host, encode_guest, encode_host, frame_length,
};

fn check_header(header: [u8; 4]) {
    let size = u32::from_be_bytes(header) as usize;
    let expected = (1..=MAX_FRAME_BYTES).contains(&size).then_some(size);
    assert_eq!(frame_length(header).ok(), expected);
}

fn framed_payload(frame: &[u8]) -> &[u8] {
    let (header, payload) = frame.split_at(4);
    assert_eq!(
        frame_length(header.try_into().expect("four-byte header")).ok(),
        Some(payload.len())
    );
    payload
}

fuzz_target!(|input: &[u8]| {
    let Some((header, payload)) = input.split_first_chunk::<4>() else {
        return;
    };
    check_header(*header);

    let host = decode_host(payload);
    let guest = decode_guest(payload);
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        assert!(host.is_err() && guest.is_err());
    }
    if let Ok(envelope) = host {
        let frame = encode_host(&envelope).expect("accepted host envelope can be encoded");
        assert_eq!(decode_host(framed_payload(&frame)).ok(), Some(envelope));
    }
    if let Ok(envelope) = guest {
        let frame = encode_guest(&envelope).expect("accepted guest envelope can be encoded");
        assert_eq!(decode_guest(framed_payload(&frame)).ok(), Some(envelope));
    }
});
