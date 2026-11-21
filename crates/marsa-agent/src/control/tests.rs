//! Exercise the control session on a bounded loopback transport.

use std::cell::Cell;

use marsa_proto::guest::{
    MAX_FRAME_BYTES, PROTOCOL_VERSION, decode_guest, encode_host,
    v1::{EngineHealthState, HelloAck, Shutdown},
};
use tokio::io::DuplexStream;

use super::*;

#[derive(Default)]
struct FakeState {
    clock: Cell<Option<i64>>,
    ready: Cell<bool>,
    shutdown_requested: Cell<bool>,
}

impl GuestState for FakeState {
    fn set_time(&self, time: SetTime) -> io::Result<()> {
        self.clock.set(Some(time.unix_seconds));
        Ok(())
    }

    async fn mark_ready(&self) -> io::Result<()> {
        assert!(self.clock.get().is_some());
        self.ready.set(true);
        Ok(())
    }

    async fn command(&self, body: host_envelope::Body) -> io::Result<Option<guest_envelope::Body>> {
        Ok(matches!(body, host_envelope::Body::Shutdown(_)).then(|| success("Shutting down")))
    }

    fn shutdown(&self) -> io::Result<()> {
        self.shutdown_requested.set(true);
        Ok(())
    }
}

fn greeting() -> Hello {
    Hello {
        protocol: Some(PROTOCOL_VERSION),
        image_version: "0.1.0".into(),
        ..Default::default()
    }
}

async fn send(client: &mut DuplexStream, id: u64, body: host_envelope::Body) {
    let envelope = HostEnvelope {
        body: Some(body),
        request_id: id,
    };
    client
        .write_all(&encode_host(&envelope).unwrap())
        .await
        .unwrap();
}

async fn receive(client: &mut DuplexStream) -> GuestEnvelope {
    let mut header = [0; 4];
    client.read_exact(&mut header).await.unwrap();
    let mut payload = vec![0; frame_length(header).unwrap()];
    client.read_exact(&mut payload).await.unwrap();
    decode_guest(&payload).unwrap()
}

async fn begin(client: &mut DuplexStream) {
    assert_eq!(
        receive(client).await.body,
        Some(guest_envelope::Body::Hello(greeting()))
    );
    send(
        client,
        0,
        host_envelope::Body::HelloAck(HelloAck {
            protocol: Some(PROTOCOL_VERSION),
        }),
    )
    .await;
    assert!(matches!(
        receive(client).await.body,
        Some(guest_envelope::Body::EngineHealth(health))
            if health.state == EngineHealthState::Starting as i32
    ));
}

#[tokio::test]
async fn negotiates_and_updates_the_clock_before_readiness() {
    let (server, mut client) = tokio::io::duplex(4 * 1024);
    let state = FakeState::default();
    let test_client = async {
        begin(&mut client).await;
        assert!(!state.ready.get());
        send(
            &mut client,
            1,
            host_envelope::Body::SetTime(SetTime {
                unix_seconds: 1_700_000_000,
                nanoseconds: 0,
            }),
        )
        .await;
        let result = receive(&mut client).await;
        assert_eq!(result.request_id, 1);
        assert!(
            matches!(result.body, Some(guest_envelope::Body::CommandResult(command)) if command.status == CommandStatus::Ok as i32)
        );
        assert_eq!(state.clock.get(), Some(1_700_000_000));
        assert!(state.ready.get());
        drop(client);
    };
    let (result, ()) = tokio::join!(
        session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
        test_client
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
}

#[tokio::test]
async fn rejects_malformed_and_over_limit_frames_without_reading_a_large_body() {
    for bad in [
        vec![0, 0, 0, 1, 0xff],
        ((MAX_FRAME_BYTES as u32) + 1).to_be_bytes().to_vec(),
    ] {
        let (server, mut client) = tokio::io::duplex(1024);
        let state = FakeState::default();
        let test_client = async {
            begin(&mut client).await;
            client.write_all(&bad).await.unwrap();
        };
        let (result, ()) = tokio::join!(
            session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
            test_client
        );
        assert_ne!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        assert!(!state.ready.get());
    }
}

#[tokio::test]
async fn shutdown_returns_success_with_its_request_id_before_session_closes() {
    let (server, mut client) = tokio::io::duplex(32);
    let state = FakeState::default();
    let test_client = async {
        begin(&mut client).await;
        assert!(!state.shutdown_requested.get());
        send(&mut client, 9, host_envelope::Body::Shutdown(Shutdown {})).await;
        let response = receive(&mut client).await;
        assert_eq!(response.request_id, 9);
        assert!(
            matches!(response.body, Some(guest_envelope::Body::CommandResult(result)) if result.status == CommandStatus::Ok as i32)
        );
        assert_eq!(client.read(&mut [0_u8; 1]).await.unwrap(), 0);
    };
    let (result, ()) = tokio::join!(
        session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
        test_client
    );
    result.unwrap();
    assert!(state.shutdown_requested.get());
}
