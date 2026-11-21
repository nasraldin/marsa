//! Exercise the public guest control boundary with untrusted protobuf payloads.

use marsa_proto::guest::v1::{
    GuestEnvelope, Hello, HelloAck, HostEnvelope, ProtocolVersion, guest_envelope, host_envelope,
};
use marsa_proto::guest::{
    MAX_FRAME_BYTES, PROTOCOL_VERSION, ProtocolError, decode_guest, decode_host, encode_guest,
    encode_host, frame_length, negotiate,
};
use prost::Message;

#[test]
fn rejects_incompatible_major_version() {
    let peer = ProtocolVersion {
        major: PROTOCOL_VERSION.major + 1,
        minor: PROTOCOL_VERSION.minor,
    };
    assert!(matches!(
        negotiate(peer),
        Err(ProtocolError::IncompatibleMajor { .. })
    ));
}

#[test]
fn negotiates_current_minor_with_newer_peer() {
    let peer = ProtocolVersion {
        major: PROTOCOL_VERSION.major,
        minor: PROTOCOL_VERSION.minor + 1,
    };
    assert_eq!(negotiate(peer).unwrap(), PROTOCOL_VERSION);
}

#[test]
fn round_trips_handshake_frames_in_both_directions() {
    let hello = GuestEnvelope {
        request_id: 0,
        body: Some(guest_envelope::Body::Hello(Hello {
            protocol: Some(PROTOCOL_VERSION),
            image_version: "1.0.0".into(),
            ..Default::default()
        })),
    };
    let encoded = encode_guest(&hello).unwrap();
    assert_eq!(
        frame_length(encoded[..4].try_into().unwrap()).unwrap(),
        encoded.len() - 4
    );
    assert_eq!(decode_guest(&encoded[4..]).unwrap(), hello);

    let ack = HostEnvelope {
        request_id: 0,
        body: Some(host_envelope::Body::HelloAck(HelloAck {
            protocol: Some(PROTOCOL_VERSION),
        })),
    };
    let encoded = encode_host(&ack).unwrap();
    assert_eq!(decode_host(&encoded[4..]).unwrap(), ack);
}

#[test]
fn rejects_oversized_prefix_before_reading_payload() {
    let too_big = u32::try_from(MAX_FRAME_BYTES + 1).unwrap();
    assert!(matches!(
        frame_length(too_big.to_be_bytes()),
        Err(ProtocolError::FrameTooLarge { .. })
    ));
    assert!(matches!(
        frame_length(0_u32.to_be_bytes()),
        Err(ProtocolError::EmptyFrame)
    ));
}

#[test]
fn rejects_malformed_or_empty_envelopes() {
    assert!(matches!(
        decode_host(&[0x0a, 0x80]),
        Err(ProtocolError::Decode(_))
    ));
    assert!(matches!(
        decode_guest(
            &GuestEnvelope {
                request_id: 42,
                body: None
            }
            .encode_to_vec()
        ),
        Err(ProtocolError::InvalidField("body"))
    ));
}

#[test]
fn rejects_overlong_version_strings_on_both_paths() {
    let message = GuestEnvelope {
        request_id: 0,
        body: Some(guest_envelope::Body::Hello(Hello {
            protocol: Some(PROTOCOL_VERSION),
            image_version: "x".repeat(129),
            ..Default::default()
        })),
    };
    assert!(matches!(
        encode_guest(&message),
        Err(ProtocolError::InvalidField("image_version"))
    ));
    assert!(matches!(
        decode_guest(&message.encode_to_vec()),
        Err(ProtocolError::InvalidField("image_version"))
    ));
}

#[test]
fn rejects_guest_results_without_request_ids_and_events_with_ids() {
    use marsa_proto::guest::v1::{EngineHealth, EngineHealthState, TrimResult};

    let result = GuestEnvelope {
        request_id: 0,
        body: Some(guest_envelope::Body::TrimResult(TrimResult::default())),
    };
    assert!(matches!(
        decode_guest(&result.encode_to_vec()),
        Err(ProtocolError::InvalidField("request_id"))
    ));

    let event = GuestEnvelope {
        request_id: 1,
        body: Some(guest_envelope::Body::EngineHealth(EngineHealth {
            state: EngineHealthState::Ready.into(),
            detail: String::new(),
        })),
    };
    assert!(matches!(
        decode_guest(&event.encode_to_vec()),
        Err(ProtocolError::InvalidField("request_id"))
    ));
}

#[test]
fn rejects_a_small_operation_that_exceeds_its_wire_limit() {
    use marsa_proto::guest::v1::TrimResult;

    let result = GuestEnvelope {
        request_id: 1,
        body: Some(guest_envelope::Body::TrimResult(TrimResult::default())),
    };
    let mut payload = result.encode_to_vec();
    // Append legal unknown varint fields to exercise the received wire length.
    for _ in 0..30 {
        payload.extend_from_slice(&[0xa0, 0x06, 0x01]);
    }
    assert!(matches!(
        decode_guest(&payload),
        Err(ProtocolError::FrameTooLarge { maximum: 64, .. })
    ));
}

#[test]
fn rejects_shape_errors_in_a_mount_path_and_a_published_port() {
    use marsa_proto::guest::v1::{MountShare, PortAction, PortEvent, Transport};

    let command = HostEnvelope {
        request_id: 1,
        body: Some(host_envelope::Body::MountShare(MountShare {
            tag: "share".into(),
            guest_path: "/Users/marsa/../.ssh".into(),
            read_only: true,
        })),
    };
    assert!(matches!(
        decode_host(&command.encode_to_vec()),
        Err(ProtocolError::InvalidField("guest_path"))
    ));

    let event = GuestEnvelope {
        request_id: 0,
        body: Some(guest_envelope::Body::PortEvent(PortEvent {
            action: PortAction::Opened.into(),
            transport: Transport::Tcp.into(),
            container_id: "ab".repeat(32),
            bind_address: "127.0.0.1".into(),
            published_port: 65_536,
            container_port: 80,
        })),
    };
    assert!(matches!(
        decode_guest(&event.encode_to_vec()),
        Err(ProtocolError::InvalidField("published_port"))
    ));
}
