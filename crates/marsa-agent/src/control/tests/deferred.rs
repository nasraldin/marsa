//! Exercise legacy gating, deferred boot, and rejection on released guests.

use super::*;
use marsa_proto::guest::v1::StartEngine;

#[tokio::test]
async fn deferred_session_releases_engine_only_on_explicit_start() {
    let (server, mut client) = tokio::io::duplex(1024);
    let state = FakeState::default();
    let test_client = async {
        begin_with(&mut client, PROTOCOL_VERSION, true).await;
        let release = || host_envelope::Body::StartEngine(StartEngine {});
        send(&mut client, 1, release()).await;
        expect_status(&mut client, 1, CommandStatus::Unavailable).await;

        send(&mut client, 2, probe()).await;
        expect_stage(&mut client, 2, EgressStage::Policy).await;
        send(
            &mut client,
            3,
            host_envelope::Body::SetTime(SetTime {
                unix_seconds: 1_700_000_000,
                nanoseconds: 0,
            }),
        )
        .await;
        expect_status(&mut client, 3, CommandStatus::Ok).await;
        assert_eq!(state.ready.get(), 0);
        send(&mut client, 4, probe()).await;
        expect_stage(&mut client, 4, EgressStage::Healthy).await;

        for id in [5, 6] {
            send(&mut client, id, release()).await;
            expect_status(&mut client, id, CommandStatus::Ok).await;
            assert_eq!(state.ready.get(), 1);
        }
        send(&mut client, 7, probe()).await;
        expect_stage(&mut client, 7, EgressStage::Policy).await;
        drop(client);
    };
    let (result, ()) = timeout(Duration::from_secs(2), async {
        tokio::join!(
            session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
            test_client
        )
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
}

async fn expect_status(client: &mut DuplexStream, id: u64, status: CommandStatus) {
    let response = response(client, id).await;
    assert!(matches!(
        response,
        guest_envelope::Body::CommandResult(command) if command.status() == status
    ));
}

async fn expect_stage(client: &mut DuplexStream, id: u64, stage: EgressStage) {
    let response = response(client, id).await;
    assert!(matches!(
        response,
        guest_envelope::Body::EgressResult(egress) if egress.stage() == stage
    ));
}

async fn response(client: &mut DuplexStream, id: u64) -> guest_envelope::Body {
    loop {
        let envelope = receive(client).await;
        if envelope.request_id != 0 {
            assert_eq!(envelope.request_id, id);
            return envelope.body.unwrap();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_stalled_probe_returns_policy_and_allows_deferred_shutdown() {
    let (server, mut client) = tokio::io::duplex(1024);
    let state = FakeState {
        probe_delay: EGRESS_TIMEOUT + Duration::from_secs(1),
        ..FakeState::default()
    };
    let test_client = async {
        begin_with(&mut client, PROTOCOL_VERSION, true).await;
        sync_clock(&mut client).await;
        send(&mut client, 2, probe()).await;
        expect_stage(&mut client, 2, EgressStage::Policy).await;
        assert_eq!(state.ready.get(), 0);
        send(&mut client, 3, host_envelope::Body::Shutdown(Shutdown {})).await;
        expect_status(&mut client, 3, CommandStatus::Ok).await;
    };
    let (result, ()) = timeout(EGRESS_TIMEOUT + Duration::from_secs(2), async {
        tokio::join!(
            session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
            test_client
        )
    })
    .await
    .unwrap();
    result.unwrap();
    assert!(state.shutdown_requested.get());
}

#[tokio::test]
async fn deferred_reconnect_cannot_probe_a_released_engine() {
    let (server, mut client) = tokio::io::duplex(1024);
    let state = FakeState {
        ready: Cell::new(1),
        ..FakeState::default()
    };
    let test_client = async {
        begin_with(&mut client, PROTOCOL_VERSION, true).await;
        sync_clock(&mut client).await;
        send(&mut client, 2, probe()).await;
        expect_stage(&mut client, 2, EgressStage::Policy).await;
        assert_eq!(state.ready.get(), 1);
        drop(client);
    };
    let (result, ()) = timeout(Duration::from_secs(2), async {
        tokio::join!(
            session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
            test_client
        )
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
}

#[tokio::test]
async fn legacy_negotiation_rejects_unexpected_new_commands() {
    let (server, mut client) = tokio::io::duplex(1024);
    let state = FakeState::default();
    let test_client = async {
        begin_with(&mut client, ProtocolVersion { major: 1, minor: 0 }, false).await;
        send(
            &mut client,
            1,
            host_envelope::Body::StartEngine(StartEngine {}),
        )
        .await;
        assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
    };
    let (result, ()) = timeout(Duration::from_secs(2), async {
        tokio::join!(
            session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
            test_client
        )
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    assert_eq!(state.ready.get(), 0);
}

fn probe() -> host_envelope::Body {
    host_envelope::Body::ProbeEgress(ProbeEgress {
        address_v4: vec![1, 1, 1, 1],
    })
}

async fn sync_clock(client: &mut DuplexStream) {
    send(
        client,
        1,
        host_envelope::Body::SetTime(SetTime {
            unix_seconds: 1_700_000_000,
            nanoseconds: 0,
        }),
    )
    .await;
    expect_status(client, 1, CommandStatus::Ok).await;
}

#[tokio::test]
async fn live_egress_requires_a_negotiated_capability_clock_and_engine_release() {
    for (minor, expected) in [(2, EgressStage::Policy), (3, EgressStage::Healthy)] {
        let (server, mut client) = tokio::io::duplex(1024);
        let state = FakeState::default();
        let live = || {
            let host_envelope::Body::ProbeEgress(target) = probe() else {
                unreachable!()
            };
            host_envelope::Body::ProbeLiveEgress(target)
        };
        let peer = async {
            begin_with(&mut client, ProtocolVersion { major: 1, minor }, true).await;
            send(&mut client, 1, live()).await;
            expect_stage(&mut client, 1, EgressStage::Policy).await;
            send(
                &mut client,
                2,
                host_envelope::Body::SetTime(SetTime {
                    unix_seconds: 1_700_000_000,
                    nanoseconds: 0,
                }),
            )
            .await;
            expect_status(&mut client, 2, CommandStatus::Ok).await;
            send(&mut client, 3, live()).await;
            expect_stage(&mut client, 3, EgressStage::Policy).await;
            send(
                &mut client,
                4,
                host_envelope::Body::StartEngine(StartEngine {}),
            )
            .await;
            expect_status(&mut client, 4, CommandStatus::Ok).await;
            send(&mut client, 5, live()).await;
            expect_stage(&mut client, 5, expected).await;
            drop(client);
        };
        let (result, ()) = timeout(Duration::from_secs(2), async {
            tokio::join!(
                session(server, greeting(), &state, "/nonexistent-marsa-test-engine"),
                peer
            )
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}
