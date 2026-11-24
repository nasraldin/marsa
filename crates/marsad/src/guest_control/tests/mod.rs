//! Exercise the production client against a scripted vsock peer.

use std::{future::poll_fn, io, sync::Arc, task::Poll, time::Duration};

use marsa_proto::guest::{
    CONTROL_PORT, DOCKER_API_PORT, MAX_FRAME_BYTES,
    v1::{
        CommandStatus, SetTime, Shutdown, Trim, TrimResult, VmStats,
        guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::fake::{AgentStep, FakeScenario};
use tokio::{sync::broadcast, time::timeout};

use super::{GuestControl, GuestControlError, GuestEvent, GuestResponse};
use crate::supervisor::{BootFailure, BootStage, GuestProbe, State, SupervisorError, Transition};

mod fixtures;
mod shutdown;

use fixtures::{
    client, command_result, docker_ping, expect, expect_events, hello, normal_session, send,
    startup, supervisor, vm,
};
const WAIT: Duration = Duration::from_secs(60);
const TEST_LIMIT: Duration = Duration::from_secs(2);

#[tokio::test(start_paused = true)]
async fn rejects_incompatible_image_with_visible_diagnostic() {
    let scenario = FakeScenario::default().with_agent(CONTROL_PORT, vec![hello(20)]);
    let (supervisor, _) = supervisor(scenario);
    let failure = supervisor.ensure_ready().await.unwrap_err();

    let SupervisorError::Boot(diagnostic) = &failure else {
        panic!("expected a boot failure");
    };
    assert_eq!(diagnostic.stage, BootStage::Handshake);
    assert!(diagnostic.detail.contains("major 20 is incompatible"));
    assert!(failure.to_string().contains("major 20 is incompatible"));
}

#[tokio::test(start_paused = true)]
async fn boot_and_resume_each_retime_ping_and_deliver_guest_events() {
    let scenario = normal_session().with_agent(DOCKER_API_PORT, docker_ping(200));
    let (supervisor, control) = supervisor(scenario);
    let mut events = control.events();
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(supervisor.snapshot().state, State::Ready);
    expect_events(&mut events).await;

    supervisor.pause().await.unwrap();
    assert!(matches!(
        control.request(HostBody::Trim(Trim {})).await,
        Err(GuestControlError::Disconnected)
    ));
    assert_eq!(supervisor.snapshot().state, State::Paused);
    supervisor.resume().await.unwrap();
    assert_eq!(supervisor.snapshot().state, State::Ready);
    expect_events(&mut events).await;
}

#[tokio::test(start_paused = true)]
async fn wake_resynchronizes_a_guest_that_did_not_pause() {
    let mut script = startup();
    script.extend([
        expect(
            2,
            HostBody::SetTime(SetTime {
                unix_seconds: 1_800_000_000,
                nanoseconds: 700,
            }),
        ),
        command_result(2, CommandStatus::Ok),
        send(
            0,
            GuestBody::Stats(VmStats {
                sample_uptime_millis: 1234,
                ..VmStats::default()
            }),
        ),
        AgentStep::Wait(WAIT),
    ]);
    let (supervisor, control) = supervisor(
        FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200)),
    );
    let mut events = control.events();
    supervisor.ensure_ready().await.unwrap();
    supervisor.host_woke().await.unwrap();

    timeout(TEST_LIMIT, async {
        while !matches!(events.recv().await.unwrap(), GuestEvent::Stats(stats) if stats.sample_uptime_millis == 1234) {}
    })
    .await
    .expect("the running guest did not receive SetTime");
    assert_eq!(supervisor.snapshot().state, State::Ready);
}

#[tokio::test(start_paused = true)]
async fn malformed_and_oversized_frames_disconnect_a_live_session() {
    for frame in [
        ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec(),
        vec![0, 0, 0, 1, 0xff],
        vec![0, 0, 0, 2, 0x08, 0x01],
        oversized_stats_frame(),
    ] {
        let mut script = startup();
        script.extend([
            AgentStep::Wait(Duration::from_millis(50)),
            AgentStep::Send(frame),
            AgentStep::Wait(WAIT),
        ]);
        let scenario = FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200));
        let (supervisor, control) = supervisor(scenario);
        let mut transitions = supervisor.events();
        supervisor.ensure_ready().await.unwrap();
        let failure = running_failure(&mut transitions).await;
        assert_eq!(failure.stage, BootStage::Running);
        assert!(matches!(
            control.request(HostBody::Trim(Trim {})).await,
            Err(GuestControlError::Disconnected)
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn bounded_handshake_and_docker_readiness() {
    let control = client();
    let vm =
        vm(FakeScenario::default().with_agent(CONTROL_PORT, vec![AgentStep::Wait(WAIT)])).await;
    let error = match control.handshake(vm.as_ref()).await {
        Ok(_) => panic!("silent guest completed handshake"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);

    let (supervisor, _) =
        supervisor(normal_session().with_agent(DOCKER_API_PORT, docker_ping(500)));
    let failure = supervisor.ensure_ready().await.unwrap_err();
    let SupervisorError::Boot(diagnostic) = failure else {
        panic!("expected a boot failure");
    };
    assert_eq!(diagnostic.stage, BootStage::Engine);
    assert!(diagnostic.timed_out);
}

#[tokio::test(start_paused = true)]
async fn correlates_trim_failures_and_disconnects_on_wrong_id() {
    let mut script = startup();
    script.extend([
        expect(2, HostBody::Trim(Trim {})),
        command_result(2, CommandStatus::Failed),
        expect(3, HostBody::Trim(Trim {})),
        command_result(2, CommandStatus::Failed),
        AgentStep::Wait(WAIT),
    ]);
    let scenario = FakeScenario::default()
        .with_agent(CONTROL_PORT, script)
        .with_agent(DOCKER_API_PORT, docker_ping(200));
    let (supervisor, control) = supervisor(scenario);
    let mut transitions = supervisor.events();
    supervisor.ensure_ready().await.unwrap();
    let answer = control.request(HostBody::Trim(Trim {})).await.unwrap();
    assert!(
        matches!(answer, GuestResponse::Command(result) if result.status == CommandStatus::Failed as i32)
    );
    assert!(control.request(HostBody::Trim(Trim {})).await.is_err());
    let failure = running_failure(&mut transitions).await;
    assert!(failure.detail.contains("request ID"));
}

fn oversized_stats_frame() -> Vec<u8> {
    // A well-formed stats envelope with unknown fields, beyond its 128-byte
    // message-specific limit but inside the global payload limit.
    let mut frame = vec![0, 0, 0, 130, 0x2a, 0x00];
    frame.extend([0x78, 0x00].repeat(64));
    frame
}

async fn running_failure(events: &mut broadcast::Receiver<Transition>) -> Arc<BootFailure> {
    timeout(WAIT + WAIT, async {
        loop {
            let event = events.recv().await.unwrap();
            if event.current == State::Failed {
                return event.failure.unwrap();
            }
        }
    })
    .await
    .unwrap()
}
#[tokio::test(start_paused = true)]
async fn partial_frame_timeout_closes_control() {
    let mut script = startup();
    script.extend([
        AgentStep::Wait(Duration::from_millis(20)),
        AgentStep::Send(vec![0, 0]),
        AgentStep::Wait(WAIT),
    ]);
    let (supervisor, _) = supervisor(
        FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200)),
    );
    let mut transitions = supervisor.events();
    supervisor.ensure_ready().await.unwrap();
    let failure = running_failure(&mut transitions).await;
    assert!(failure.detail.contains("reading a partial frame"));
}

#[tokio::test(start_paused = true)]
async fn bounds_queued_requests_and_closes_on_missing_response() {
    let mut script = startup();
    script.extend([
        expect(2, HostBody::Trim(Trim {})),
        send(0, GuestBody::Stats(VmStats::default())),
        AgentStep::Wait(WAIT),
    ]);
    let (supervisor, control) = supervisor(
        FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200)),
    );
    let mut events = control.events();
    let mut transitions = supervisor.events();
    supervisor.ensure_ready().await.unwrap();

    let mut requests = (0..=super::session::REQUEST_CAPACITY)
        .map(|_| Box::pin(control.request(HostBody::Trim(Trim {}))))
        .collect::<Vec<_>>();
    poll_fn(|cx| {
        assert!(requests[0].as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    // Seeing the stats frame confirms the scripted peer received request #2.
    while !matches!(events.recv().await.unwrap(), GuestEvent::Stats(_)) {}
    poll_fn(|cx| {
        for request in &mut requests[1..] {
            assert!(request.as_mut().poll(cx).is_pending());
        }
        Poll::Ready(())
    })
    .await;
    assert!(matches!(
        control.request(HostBody::Trim(Trim {})).await,
        Err(GuestControlError::Busy)
    ));
    for request in requests {
        assert!(matches!(request.await, Err(GuestControlError::TimedOut(_))));
    }
    let failure = running_failure(&mut transitions).await;
    assert!(failure.detail.contains("command result"));
}

#[tokio::test(start_paused = true)]
async fn interleaved_command_preserves_partially_read_event() {
    let mut script = startup();
    let AgentStep::Send(stats) = send(0, GuestBody::Stats(VmStats::default())) else {
        unreachable!();
    };
    script.extend([
        AgentStep::Send(stats[..2].to_vec()),
        expect(2, HostBody::Trim(Trim {})),
        AgentStep::Send(stats[2..].to_vec()),
        send(
            2,
            GuestBody::TrimResult(TrimResult {
                bytes_trimmed: 512,
                data_disk_used_bytes: 1024,
            }),
        ),
        AgentStep::Wait(WAIT),
    ]);
    let (supervisor, control) = supervisor(
        FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200)),
    );
    let mut events = control.events();
    supervisor.ensure_ready().await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(matches!(
        control.request(HostBody::Trim(Trim {})).await.unwrap(),
        GuestResponse::Trim(result) if result.bytes_trimmed == 512
    ));
    while !matches!(events.recv().await.unwrap(), GuestEvent::Stats(_)) {}
}

#[tokio::test(start_paused = true)]
async fn rejects_mismatched_result_type_on_the_right_request_id() {
    let mut script = startup();
    script.extend([
        expect(2, HostBody::Trim(Trim {})),
        command_result(2, CommandStatus::Ok),
        AgentStep::Wait(WAIT),
    ]);
    let (supervisor, control) = supervisor(
        FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200)),
    );
    let mut transitions = supervisor.events();
    supervisor.ensure_ready().await.unwrap();
    assert!(control.request(HostBody::Trim(Trim {})).await.is_err());
    let failure = running_failure(&mut transitions).await;
    assert!(failure.detail.contains("trim success requires TrimResult"));
}

#[tokio::test(start_paused = true)]
async fn requests_guest_shutdown_and_requires_its_acknowledgement() {
    for status in [CommandStatus::Ok, CommandStatus::Failed] {
        let mut script = startup();
        script.extend([
            expect(2, HostBody::Shutdown(Shutdown {})),
            command_result(2, status),
            AgentStep::Wait(WAIT),
        ]);
        let vm = vm(FakeScenario::default().with_agent(CONTROL_PORT, script)).await;
        let mut session = client().handshake(vm.as_ref()).await.unwrap();
        let outcome = session.shutdown().await;
        if status == CommandStatus::Ok {
            assert!(outcome.is_ok());
        } else {
            assert!(
                outcome
                    .unwrap_err()
                    .to_string()
                    .contains("rejected shutdown")
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_native_failure_after_shutdown_ack_is_reported() {
    let mut script = startup();
    script.extend([
        expect(2, HostBody::Shutdown(Shutdown {})),
        command_result(2, CommandStatus::Ok),
        AgentStep::Wait(WAIT),
    ]);
    let scenario = FakeScenario::default()
        .with_agent(CONTROL_PORT, script)
        .with_agent(DOCKER_API_PORT, docker_ping(200))
        .with_exit_after(Duration::from_millis(100));
    let (supervisor, _) = supervisor(scenario);
    supervisor.ensure_ready().await.unwrap();

    let error = supervisor.stop().await.unwrap_err();
    assert!(error.to_string().contains("scripted guest exit"));
    let snapshot = supervisor.snapshot();
    assert_eq!(snapshot.state, State::Failed);
    assert_eq!(snapshot.failure.unwrap().stage, BootStage::Shutdown);
}

#[tokio::test(start_paused = true)]
async fn paused_stop_reconnects_to_request_shutdown_without_publish_ready() {
    let mut script = startup();
    script.extend([
        expect(2, HostBody::Shutdown(Shutdown {})),
        command_result(2, CommandStatus::Ok),
        send(
            0,
            GuestBody::Stats(VmStats {
                sample_uptime_millis: 700,
                ..VmStats::default()
            }),
        ),
        AgentStep::Wait(WAIT),
    ]);
    let scenario = FakeScenario::default()
        .with_agent(CONTROL_PORT, script)
        .with_agent(DOCKER_API_PORT, docker_ping(200));
    let (supervisor, control) = supervisor(scenario);
    supervisor.ensure_ready().await.unwrap();
    supervisor.pause().await.unwrap();
    let mut events = supervisor.events();
    let mut messages = control.events();

    supervisor.stop().await.unwrap();
    let first = events.try_recv().unwrap();
    let second = events.try_recv().unwrap();
    assert_eq!(
        (first.previous, first.current),
        (State::Paused, State::Stopping)
    );
    assert_eq!(
        (second.previous, second.current),
        (State::Stopping, State::Stopped)
    );
    assert!(events.try_recv().is_err());

    let mut requested_shutdown = false;
    while let Ok(event) = messages.try_recv() {
        if matches!(event, GuestEvent::Stats(stats) if stats.sample_uptime_millis == 700) {
            requested_shutdown = true;
        }
    }
    assert!(requested_shutdown);
}

#[tokio::test]
async fn idle_observation_decodes_running_paused_and_restarting_workloads() {
    let count =
        sample_workloads(br#"[{"State":"running"},{"State":"paused"},{"State":"restarting"}]"#)
            .await
            .unwrap();
    assert_eq!(count, 3);
    assert_eq!(sample_workloads(b"[]").await.unwrap(), 0);
}

#[tokio::test]
async fn idle_observation_rejects_unexpected_state_and_oversized_bodies() {
    assert!(sample_workloads(br#"[{"State":"exited"}]"#).await.is_err());
    let mut oversized = vec![b' '; 64 * 1024];
    oversized.extend_from_slice(b"[]");
    assert!(sample_workloads(&oversized).await.is_err());
}

async fn sample_workloads(json: &[u8]) -> io::Result<u32> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        json.len()
    )
    .into_bytes();
    response.extend_from_slice(json);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    let request = b"GET /v1.57/containers/json?all=1&filters=%7B%22status%22%3A%5B%22running%22%2C%22paused%22%2C%22restarting%22%5D%7D HTTP/1.1\r\n";
    let scenario = FakeScenario::default().with_agent(
        DOCKER_API_PORT,
        vec![
            AgentStep::Expect(request.to_vec()),
            AgentStep::Send(response),
        ],
    );
    let vm = vm(scenario).await;
    GuestControl::default()
        .active_containers(vm.as_ref())
        .await
        .unwrap()
}

mod network_mode;
fn boot_proxy_script(
    policy: &marsa_proto::guest::v1::ConfigureNetwork,
    outcome: GuestBody,
) -> Vec<AgentStep> {
    use marsa_proto::guest::{
        PROTOCOL_VERSION,
        v1::{Hello, HelloAck, SetTime},
    };

    vec![
        send(
            0,
            GuestBody::Hello(Hello {
                protocol: Some(PROTOCOL_VERSION),
                image_version: "scripted-agent".into(),
                ..Default::default()
            }),
        ),
        expect(
            0,
            HostBody::HelloAck(HelloAck {
                protocol: Some(PROTOCOL_VERSION),
                defer_engine_start: true,
            }),
        ),
        expect(
            1,
            HostBody::SetTime(SetTime {
                unix_seconds: 1_800_000_000,
                nanoseconds: 700,
            }),
        ),
        command_result(1, CommandStatus::Ok),
        expect(2, HostBody::ConfigureNetwork(policy.clone())),
        send(2, outcome),
    ]
}

fn boot_proxy() -> marsa_proto::guest::v1::ConfigureNetwork {
    use marsa_proto::guest::v1::{ConfigureNetwork, Proxy};
    ConfigureNetwork {
        proxy: Some(Proxy {
            http_url: Some("http://proxy.example.invalid:8080".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test(start_paused = true)]
async fn approved_proxy_is_acknowledged_before_engine_release() {
    use marsa_proto::guest::v1::{CommandResult, EngineHealthState, StartEngine};

    let policy = boot_proxy();
    let mut script = boot_proxy_script(
        &policy,
        GuestBody::CommandResult(CommandResult {
            status: CommandStatus::Ok.into(),
            ..Default::default()
        }),
    );
    script.extend([
        expect(3, HostBody::StartEngine(StartEngine {})),
        command_result(3, CommandStatus::Ok),
        send(0, fixtures::health(EngineHealthState::Ready)),
        AgentStep::Wait(WAIT),
    ]);
    let vm = vm(FakeScenario::default()
        .with_agent(CONTROL_PORT, script)
        .with_agent(DOCKER_API_PORT, docker_ping(200)))
    .await;
    let mut control = client()
        .handshake_for_boot(vm.as_ref(), Some(&policy))
        .await
        .unwrap();
    control.engine_ready(vm.as_ref()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rejected_boot_proxy_prevents_ready_without_exposing_guest_detail() {
    use marsa_proto::guest::v1::CommandResult;

    let policy = boot_proxy();
    let detail = "untrusted guest response with http://sensitive.example.invalid:8080";
    let mut script = boot_proxy_script(
        &policy,
        GuestBody::CommandResult(CommandResult {
            status: CommandStatus::Unavailable.into(),
            detail: detail.into(),
        }),
    );
    script.push(AgentStep::Wait(WAIT));
    let vm = vm(FakeScenario::default().with_agent(CONTROL_PORT, script)).await;
    let error = match client()
        .handshake_for_boot(vm.as_ref(), Some(&policy))
        .await
    {
        Ok(_) => panic!("guest policy rejection must abort activation"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(!error.to_string().contains("sensitive.example.invalid"));
}

#[tokio::test(start_paused = true)]
async fn approved_proxy_rejects_legacy_guest_before_acknowledging_startup() {
    let vm =
        vm(FakeScenario::default().with_agent(CONTROL_PORT, vec![hello(1), AgentStep::Wait(WAIT)]))
            .await;
    let policy = boot_proxy();
    let result = client()
        .handshake_for_boot(vm.as_ref(), Some(&policy))
        .await;
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::Unsupported));
}
