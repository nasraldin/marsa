//! Check bounded framing and validation for isolated bind-policy traffic.

use marsa_proto::guest::{
    BIND_AUTHORIZATION_VERSION, MAX_BIND_AUTHORIZATION_FRAME_BYTES,
    bind_authorization_frame_length, decode_bind_authorization_request,
    decode_bind_authorization_response, encode_bind_authorization_request,
    encode_bind_authorization_response,
    v1::{BindAuthorizationDecision, BindAuthorizationRequest, BindAuthorizationResponse},
};

#[test]
fn authorization_round_trips_only_bind_sources_and_a_correlated_reply() {
    let request = BindAuthorizationRequest {
        protocol: Some(BIND_AUTHORIZATION_VERSION),
        request_id: 7,
        sources: vec!["/Users/test/مجلد".into(), "/Users/test/other".into()],
    };
    let frame = encode_bind_authorization_request(&request).unwrap();
    assert_eq!(
        bind_authorization_frame_length(frame[..4].try_into().unwrap()).unwrap(),
        frame.len() - 4
    );
    assert_eq!(
        decode_bind_authorization_request(&frame[4..]).unwrap(),
        request
    );

    let response = BindAuthorizationResponse {
        protocol: request.protocol,
        request_id: request.request_id,
        decision: BindAuthorizationDecision::Deny.into(),
        message: "Share the folder from Marsa settings.".into(),
    };
    let frame = encode_bind_authorization_response(&response).unwrap();
    assert_eq!(
        decode_bind_authorization_response(&frame[4..]).unwrap(),
        response
    );
}

#[test]
fn rejects_oversized_frames_and_malformed_queries_or_decisions() {
    assert!(bind_authorization_frame_length([0; 4]).is_err());
    assert!(
        bind_authorization_frame_length(
            ((MAX_BIND_AUTHORIZATION_FRAME_BYTES + 1) as u32).to_be_bytes()
        )
        .is_err()
    );
    for sources in [
        vec![],
        vec!["relative".into()],
        vec!["/bad\npath".into()],
        vec!["/x".into(); 65],
    ] {
        let request = BindAuthorizationRequest {
            protocol: Some(BIND_AUTHORIZATION_VERSION),
            request_id: 1,
            sources,
        };
        assert!(encode_bind_authorization_request(&request).is_err());
    }
    for (decision, message) in [
        (0, ""),
        (1, "unexpected message"),
        (2, ""),
        (2, "control\ncharacter"),
    ] {
        let response = BindAuthorizationResponse {
            protocol: Some(BIND_AUTHORIZATION_VERSION),
            request_id: 1,
            decision,
            message: message.into(),
        };
        assert!(encode_bind_authorization_response(&response).is_err());
    }
}
