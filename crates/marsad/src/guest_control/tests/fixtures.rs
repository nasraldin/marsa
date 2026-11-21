//! Script helpers for the protocol and the fake VM.

use std::{
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use marsa_proto::guest::{
    CONTROL_PORT, PROTOCOL_VERSION, encode_guest, encode_host,
    v1::{
        CommandResult, CommandStatus, EngineHealth, EngineHealthState, GuestEnvelope, Hello,
        HelloAck, HostEnvelope, PortAction, PortEvent, ProtocolVersion, SetTime, Transport,
        VmStats, guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::{
    DiskSpec, Hypervisor, NetworkSpec, Vm, VmSpec,
    fake::{AgentStep, FakeHypervisor, FakeScenario},
};
use tokio::{sync::broadcast, time::timeout};

use super::{GuestControl, GuestEvent, TEST_LIMIT, WAIT};
use crate::supervisor::{Supervisor, SupervisorConfig};

const NOW: Duration = Duration::new(1_800_000_000, 700);

pub(super) fn client() -> GuestControl {
    GuestControl {
        clock: || UNIX_EPOCH + NOW,
        ..GuestControl::default()
    }
}

pub(super) fn spec() -> VmSpec {
    VmSpec {
        cpus: 2,
        memory_bytes: 512 * 1024 * 1024,
        kernel: "/fixture/Image".into(),
        initramfs: None,
        cmdline: "console=hvc0".into(),
        root_disk: DiskSpec {
            path: "/fixture/rootfs.img".into(),
            read_only: true,
        },
        data_disk: DiskSpec {
            path: "/fixture/data.img".into(),
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Nat,
        console_log: "/fixture/console.log".into(),
    }
}

pub(super) async fn vm(scenario: FakeScenario) -> Box<dyn Vm> {
    let mut vm = FakeHypervisor::new(scenario).create(spec()).await.unwrap();
    vm.start().await.unwrap();
    vm
}

pub(super) fn supervisor(scenario: FakeScenario) -> (Supervisor, GuestControl) {
    let control = client();
    let config = SupervisorConfig {
        boot_timeout: TEST_LIMIT,
        stop_timeout: TEST_LIMIT,
        idle_timeout: WAIT,
        idle_stop: false,
        max_retries: 0,
        initial_backoff: Duration::ZERO,
        maximum_backoff: Duration::ZERO,
    };
    (
        Supervisor::spawn(
            Arc::new(FakeHypervisor::new(scenario)),
            Arc::new(control.clone()),
            spec(),
            config,
        ),
        control,
    )
}

pub(super) fn hello(major: u32) -> AgentStep {
    send(
        0,
        GuestBody::Hello(Hello {
            protocol: Some(ProtocolVersion {
                major,
                minor: u32::MAX,
            }),
            image_version: "scripted-agent".into(),
            ..Hello::default()
        }),
    )
}

pub(super) fn send(request_id: u64, body: GuestBody) -> AgentStep {
    AgentStep::Send(
        encode_guest(&GuestEnvelope {
            request_id,
            body: Some(body),
        })
        .unwrap(),
    )
}

pub(super) fn expect(request_id: u64, body: HostBody) -> AgentStep {
    AgentStep::Expect(
        encode_host(&HostEnvelope {
            request_id,
            body: Some(body),
        })
        .unwrap(),
    )
}

pub(super) fn command_result(id: u64, status: CommandStatus) -> AgentStep {
    send(
        id,
        GuestBody::CommandResult(CommandResult {
            status: status as i32,
            detail: String::new(),
        }),
    )
}

pub(super) fn health(state: EngineHealthState) -> GuestBody {
    GuestBody::EngineHealth(EngineHealth {
        state: state as i32,
        detail: String::new(),
    })
}

pub(super) fn startup() -> Vec<AgentStep> {
    vec![
        hello(PROTOCOL_VERSION.major),
        expect(
            0,
            HostBody::HelloAck(HelloAck {
                protocol: Some(PROTOCOL_VERSION),
            }),
        ),
        send(0, health(EngineHealthState::Starting)),
        expect(
            1,
            HostBody::SetTime(SetTime {
                unix_seconds: NOW.as_secs() as i64,
                nanoseconds: NOW.subsec_nanos(),
            }),
        ),
        command_result(1, CommandStatus::Ok),
        send(0, health(EngineHealthState::Ready)),
    ]
}

pub(super) fn normal_session() -> FakeScenario {
    let mut script = startup();
    script.extend([
        send(
            0,
            GuestBody::PortEvent(PortEvent {
                action: PortAction::Opened as i32,
                transport: Transport::Tcp as i32,
                container_id: "a".repeat(64),
                bind_address: "127.0.0.1".into(),
                published_port: 8080,
                container_port: 80,
            }),
        ),
        send(
            0,
            GuestBody::Stats(VmStats {
                sample_uptime_millis: 250,
                ..VmStats::default()
            }),
        ),
        AgentStep::Wait(WAIT),
    ]);
    FakeScenario::default().with_agent(CONTROL_PORT, script)
}

pub(super) fn docker_ping(status: u16) -> Vec<AgentStep> {
    vec![
        AgentStep::Expect(
            b"GET /_ping HTTP/1.0\r\nHost: docker\r\nConnection: close\r\n\r\n".to_vec(),
        ),
        AgentStep::Send(
            format!("HTTP/1.1 {status} Docker\r\nContent-Length: 2\r\n\r\nOK").into_bytes(),
        ),
    ]
}

pub(super) async fn expect_events(events: &mut broadcast::Receiver<GuestEvent>) {
    for state in [EngineHealthState::Starting, EngineHealthState::Ready] {
        assert!(
            matches!(timeout(TEST_LIMIT, events.recv()).await.unwrap().unwrap(), GuestEvent::Health(health) if health.state == state as i32)
        );
    }
    assert!(
        matches!(timeout(TEST_LIMIT, events.recv()).await.unwrap().unwrap(), GuestEvent::Port(port) if port.published_port == 8080)
    );
    assert!(
        matches!(timeout(TEST_LIMIT, events.recv()).await.unwrap().unwrap(), GuestEvent::Stats(stats) if stats.sample_uptime_millis == 250)
    );
}
