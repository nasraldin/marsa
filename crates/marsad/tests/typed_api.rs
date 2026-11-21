//! Drive the generated clients against the real supervisor and a fake hypervisor.

#![cfg(unix)]

use std::{io, sync::Arc, time::Duration};

use hyper_util::rt::TokioIo;
use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::{
    guest::v1 as guest,
    host::v1::{
        CapabilitiesRequest, DaemonEvent, EngineActionRequest, EngineHealthState, EngineState,
        EngineStatusRequest, GetSettingsRequest, SubscribeRequest, UpdateSettingsRequest,
        VmStatsRequest, WatchSettingsRequest, daemon_event::Body, engine_client::EngineClient,
        events_client::EventsClient, meta_client::MetaClient, settings_client::SettingsClient,
        stats_client::StatsClient,
    },
};
use marsa_vm::{
    DiskSpec, NetworkSpec, Vm, VmSpec, VsockStream, async_trait,
    fake::{AgentStep, FakeHypervisor, FakeScenario},
};
use marsad::{
    supervisor::{GuestProbe, GuestSession, Supervisor, SupervisorConfig},
    typed_api::TypedApi,
};
use tempfile::TempDir;
use tokio::{
    io::AsyncReadExt,
    net::{UnixListener, UnixStream},
    sync::oneshot,
    time::timeout,
};
use tonic::{
    Code, Streaming,
    transport::{Channel, Endpoint},
};
use tower::service_fn;

const WAIT: Duration = Duration::from_secs(3);

#[tokio::test]
async fn restart_has_one_activation_and_lifecycle_events_follow_the_same_cursor() {
    let fixture = Fixture::new(vec![AgentStep::Send(b"HR".to_vec())]).await;
    let mut engine = EngineClient::new(fixture.channel.clone());
    let capabilities = MetaClient::new(fixture.channel.clone())
        .capabilities(CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(capabilities.backends[0].backend, "fake");
    assert!(capabilities.backends[0].pause_resume);
    let mut events = fixture.events().await;
    let initial = next(&mut events).await;
    assert!(
        matches!(&initial.body, Some(Body::Snapshot(s)) if s.engine.as_ref().unwrap().state() == EngineState::Stopped)
    );
    assert_eq!(
        engine.start(action(1)).await.unwrap().into_inner().state(),
        EngineState::Ready
    );
    let mut previous = initial;
    for state in [
        EngineState::Starting,
        EngineState::Booting,
        EngineState::Ready,
    ] {
        previous = next_state(&mut events, previous, state).await;
    }
    let network = next(&mut events).await;
    assert_eq!(network.sequence, previous.sequence + 1);
    assert!(matches!(&network.body, Some(Body::Network(status)) if
        status.active_mode() == marsa_proto::host::v1::NetworkMode::Nat
            && status.reason.as_deref() == Some("probe_unavailable")));
    previous = network;
    assert_eq!(
        engine.start(action(1)).await.unwrap().into_inner().state(),
        EngineState::Ready
    );
    assert_eq!(
        engine.start(action(2)).await.unwrap().into_inner().state(),
        EngineState::Ready
    );

    let mut concurrent = engine.clone();
    let (a, b) = tokio::join!(engine.restart(action(3)), concurrent.restart(action(3)));
    assert_eq!(a.unwrap().into_inner(), b.unwrap().into_inner());
    for state in [
        EngineState::Stopping,
        EngineState::Stopped,
        EngineState::Starting,
        EngineState::Booting,
        EngineState::Ready,
    ] {
        previous = next_state(&mut events, previous, state).await;
    }
    assert_eq!(
        engine.stop(action(3)).await.unwrap_err().code(),
        Code::AlreadyExists
    );
    assert_eq!(
        engine.stop(action(4)).await.unwrap().into_inner().state(),
        EngineState::Stopped
    );
    assert_eq!(
        engine.stop(action(4)).await.unwrap().into_inner().state(),
        EngineState::Stopped
    );
    for state in [EngineState::Stopping, EngineState::Stopped] {
        previous = next_state(&mut events, previous, state).await;
    }
    let cleared = next(&mut events).await;
    assert_eq!(cleared.sequence, previous.sequence + 1);
    assert!(matches!(&cleared.body, Some(Body::Network(status)) if
        status.active_mode() == marsa_proto::host::v1::NetworkMode::Unspecified));
    assert_eq!(
        next(&mut fixture.events().await).await.sequence,
        cleared.sequence
    );
}

#[tokio::test]
async fn stopping_cancels_a_pending_start_without_waiting_for_its_boot_deadline() {
    let fixture = Fixture::new(vec![AgentStep::Wait(Duration::from_secs(20))]).await;
    let mut engine = EngineClient::new(fixture.channel.clone());
    let mut events = fixture.events().await;
    let initial = next(&mut events).await;
    let mut starter = engine.clone();
    let start = tokio::spawn(async move { starter.start(action(1)).await });
    next_state(&mut events, initial, EngineState::Starting).await;
    let result = timeout(WAIT, engine.stop(action(2)))
        .await
        .expect("stop waited for boot timeout");
    assert_eq!(result.unwrap().into_inner().state(), EngineState::Stopped);
    assert_eq!(start.await.unwrap().unwrap_err().code(), Code::Unavailable);
}

#[tokio::test]
async fn streams_use_guest_health_and_stats_and_clear_them_when_the_vm_stops() {
    let fixture = Fixture::new(vec![AgentStep::Send(b"HR".to_vec())]).await;
    let mut engine = EngineClient::new(fixture.channel.clone());
    engine.start(action(1)).await.unwrap();
    let mut events = fixture.events().await;
    let initial = next(&mut events).await;
    let mut stats = StatsClient::new(fixture.channel.clone())
        .vm_stats(VmStatsRequest {})
        .await
        .unwrap()
        .into_inner();
    fixture.api.publish_guest_health(&guest::EngineHealth {
        state: guest::EngineHealthState::Unhealthy.into(),
        detail: "example guest diagnostic".into(),
    });
    let sample = guest::VmStats {
        memory_used_bytes: 64,
        memory_total_bytes: 128,
        data_disk_used_bytes: 256,
        data_disk_total_bytes: 512,
        ..Default::default()
    };
    fixture.api.publish_guest_stats(guest::VmStats {
        memory_used_bytes: 200,
        ..sample
    });
    fixture.api.publish_guest_stats(sample);
    let event = next_state(&mut events, initial, EngineState::Ready).await;
    assert!(
        matches!(event.body, Some(Body::Engine(s)) if s.guest_health() == EngineHealthState::Unhealthy)
    );
    assert_eq!(next(&mut stats).await.memory_used_bytes, 64);
    engine.stop(action(2)).await.unwrap();
    let stopped = engine
        .status(EngineStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stopped.guest_health(), EngineHealthState::Unspecified);
    let mut later = StatsClient::new(fixture.channel.clone())
        .vm_stats(VmStatsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(
        timeout(Duration::from_millis(50), later.message())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn settings_apply_idle_policy_without_requiring_restart() {
    let fixture = Fixture::new(vec![AgentStep::Send(b"HR".to_vec())]).await;
    let mut engine = EngineClient::new(fixture.channel.clone());
    engine.start(action(1)).await.unwrap();
    let mut settings = SettingsClient::new(fixture.channel.clone());
    let mut watch = settings
        .watch(WatchSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let initial = next(&mut watch).await;
    let update = settings
        .update(UpdateSettingsRequest {
            request_id: action(2).request_id,
            expected_revision: initial.revision,
            settings_toml: "[vm]\nidle_timeout = 1\n".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(next(&mut watch).await, update);
    assert!(!update.restart_required);
    assert_eq!(
        settings
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner()
            .revision,
        initial.revision + 1
    );
    let mut events = fixture.events().await;
    let initial = next(&mut events).await;
    fixture.supervisor.report_containers(0).await.unwrap();
    let idle = next_state(&mut events, initial, EngineState::Idle).await;
    let stopping = next_state(&mut events, idle, EngineState::Stopping).await;
    next_state(&mut events, stopping, EngineState::Stopped).await;
}

struct Fixture {
    api: TypedApi,
    supervisor: Supervisor,
    channel: Channel,
    _home: TempDir,
    _shutdown: oneshot::Sender<()>,
}

impl Fixture {
    async fn new(steps: Vec<AgentStep>) -> Self {
        let home = TempDir::new_in("/tmp").unwrap();
        let paths = MarsaPaths::from_home(home.path()).unwrap();
        std::fs::create_dir(paths.root()).unwrap();
        let path = home.path().join("grpc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let api =
            TypedApi::new(paths, nix::unistd::geteuid().as_raw(), Settings::default()).unwrap();
        let (api, supervisor) = api.with_engine(
            Arc::new(FakeHypervisor::new(
                FakeScenario::default().with_agent(5000, steps),
            )),
            Arc::new(FixtureProbe),
            VmSpec {
                cpus: 2,
                memory_bytes: 512 * 1024 * 1024,
                kernel: "/fixture/Image".into(),
                initramfs: None,
                cmdline: "console=hvc0".into(),
                root_disk: DiskSpec {
                    path: "/fixture/rootfs".into(),
                    read_only: true,
                },
                data_disk: DiskSpec {
                    path: "/fixture/data".into(),
                    read_only: false,
                },
                shares: Vec::new(),
                network: NetworkSpec::Nat,
                console_log: home.path().join("console"),
            },
            SupervisorConfig {
                boot_timeout: Duration::from_secs(30),
                stop_timeout: Duration::from_secs(1),
                idle_timeout: Duration::from_secs(300),
                idle_stop: true,
                max_retries: 0,
                initial_backoff: Duration::ZERO,
                maximum_backoff: Duration::ZERO,
            },
        );
        let (shutdown, stop) = oneshot::channel();
        let serving = api.clone();
        tokio::spawn(async move {
            serving
                .serve(listener, async {
                    let _ = stop.await;
                })
                .await
                .unwrap();
        });
        let channel = Endpoint::from_static("http://localhost")
            .connect_with_connector(service_fn(move |_| {
                let path = path.clone();
                async move { UnixStream::connect(path).await.map(TokioIo::new) }
            }))
            .await
            .unwrap();
        Self {
            api,
            supervisor,
            channel,
            _home: home,
            _shutdown: shutdown,
        }
    }

    async fn events(&self) -> Streaming<DaemonEvent> {
        EventsClient::new(self.channel.clone())
            .subscribe(SubscribeRequest {})
            .await
            .unwrap()
            .into_inner()
    }
}

struct FixtureProbe;
struct FixtureSession(VsockStream);

#[async_trait]
impl GuestProbe for FixtureProbe {
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        let mut stream = vm.connect_vsock(5000).await.map_err(io::Error::other)?;
        assert_eq!(stream.read_u8().await?, b'H');
        Ok(Box::new(FixtureSession(stream)))
    }
}

#[async_trait]
impl GuestSession for FixtureSession {
    async fn engine_ready(&mut self, _: &dyn Vm) -> io::Result<()> {
        assert_eq!(self.0.read_u8().await?, b'R');
        Ok(())
    }
}

fn action(id: u32) -> EngineActionRequest {
    EngineActionRequest {
        request_id: format!("20000000-0000-4000-8000-{id:012}"),
    }
}

async fn next<T>(stream: &mut Streaming<T>) -> T {
    timeout(WAIT, stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

async fn next_state(
    events: &mut Streaming<DaemonEvent>,
    previous: DaemonEvent,
    expected: EngineState,
) -> DaemonEvent {
    let mut previous = previous;
    loop {
        let event = next(events).await;
        assert_eq!(event.stream_id, previous.stream_id);
        assert_eq!(event.sequence, previous.sequence + 1);
        match event.body.as_ref() {
            Some(Body::Network(_)) => previous = event,
            Some(Body::Engine(status)) => {
                assert_eq!(status.state(), expected);
                return event;
            }
            _ => panic!("unexpected event while waiting for {expected:?}"),
        }
    }
}
