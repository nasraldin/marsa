//! Drive the generated clients against the real supervisor and a fake hypervisor.

#![cfg(unix)]

use std::{io, sync::Arc, time::Duration};

use hyper_util::rt::TokioIo;
use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::{
    guest::v1 as guest,
    host::v1::{
        AddShareRequest, CapabilitiesRequest, DaemonEvent, EngineActionRequest, EngineHealthState,
        EngineState, EngineStatusRequest, GetSettingsRequest, ListSharesRequest,
        RemoveShareRequest, SubscribeRequest, UpdateSettingsRequest, VmStatsRequest,
        WatchSettingsRequest, daemon_event::Body, engine_client::EngineClient,
        events_client::EventsClient, meta_client::MetaClient, settings_client::SettingsClient,
        shares_client::SharesClient, stats_client::StatsClient,
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

#[tokio::test]
async fn collects_one_private_archive_and_omits_secrets() {
    use marsa_proto::host::v1::{
        CollectDiagnosticsRequest, diagnostics_client::DiagnosticsClient,
        diagnostics_server::Diagnostics,
    };
    use std::{collections::BTreeMap, fs, io::Read, os::unix::fs::PermissionsExt as _, path::Path};

    let fixture = Fixture::new(vec![AgentStep::Send(b"HR".to_vec())]).await;
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    for directory in [paths.root().to_owned(), paths.logs_dir(), paths.vm_dir()] {
        if !directory.exists() {
            fs::create_dir(&directory).unwrap();
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let write_log = |path: &Path, text: &str| {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    };
    let daemon_log = paths.logs_dir().join("marsad.2026-11-22.jsonl");
    write_log(
        &daemon_log,
        concat!(
            "{\"level\":\"INFO\",\"fields\":{\"message\":\"daemon started\",\"token\":\"opaque_daemon_credential\"}}\n",
            "{\"level\":\"WARN\",\"fields\":{\"message\":\"AUTHORIZATION=opaque_env_value https://user:secret_url@example.invalid\"}}\n",
        ),
    );
    std::os::unix::fs::symlink(&daemon_log, paths.daemon_log()).unwrap();
    write_log(
        &paths.console_log(),
        concat!(
            "[    0.00000] Kernel command line: opaque_guest_parameters\n",
            "marsa-init stage=data_disk duration_ms=437\n",
            "marsa-init: AWS_SECRET_ACCESS_KEY=opaque_console_secret\n",
            "time=\"2026-11-22T02:00:00.001Z\" level=info msg=\"Docker daemon\" version=29.0.0\n",
            "time=\"2026-11-22T02:00:00.002Z\" level=error msg=\"Bearer opaque_engine_secret\"\n",
            "\x1b[36mINFO\x1b[0m[2026-11-22T02:00:00.003Z] Daemon has completed initialization token=opaque_color_secret\n",
        ),
    );
    let project = fixture.home.path().join("opaque_host_path");
    fs::create_dir(&project).unwrap();
    let mut settings = SettingsClient::new(fixture.channel.clone());
    let snapshot = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    settings
        .update(UpdateSettingsRequest {
            request_id: "70000000-0000-4000-8000-000000000002".into(),
            expected_revision: snapshot.revision,
            settings_toml: format!(
                concat!(
                    "shares = [\"{}\"]\n",
                    "[network]\ndns = [\"192.0.2.17\"]\n",
                    "[network.proxy]\nhttps = \"https://opaque-proxy-host.example.invalid:88\"\n",
                ),
                project.display(),
            ),
        })
        .await
        .unwrap();

    let mut engine = EngineClient::new(fixture.channel.clone());
    engine.start(action(91)).await.unwrap();
    engine.stop(action(92)).await.unwrap();

    // The collector uses the same authenticated UDS service as the other RPCs.
    let request = CollectDiagnosticsRequest {
        request_id: "70000000-0000-4000-8000-000000000003".into(),
        include_agent_audit: false,
    };
    assert_eq!(
        fixture
            .api
            .collect(tonic::Request::new(request.clone()))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let mut diagnostics = DiagnosticsClient::new(fixture.channel.clone());
    let output = diagnostics
        .collect(request.clone())
        .await
        .unwrap()
        .into_inner();
    let replay = diagnostics
        .collect(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        diagnostics
            .collect(CollectDiagnosticsRequest {
                include_agent_audit: true,
                ..request
            })
            .await
            .unwrap_err()
            .code(),
        Code::AlreadyExists
    );
    assert_eq!(output, replay);
    let path = Path::new(&output.bundle_path);
    assert!(path.starts_with(paths.root()));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    let mut archive = tar::Archive::new(fs::File::open(path).unwrap());
    let entries: BTreeMap<_, _> = archive
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().into_owned();
            let mut contents = String::new();
            entry.read_to_string(&mut contents).unwrap();
            (name, contents)
        })
        .collect();
    for (name, expected) in [
        ("daemon-log.json", "daemon_started"),
        ("console-log.json", "data_disk"),
        ("console-log.json", "kernel_command_line_redacted"),
        ("engine-log.json", "docker_daemon"),
        ("engine-log.json", "docker_initialized"),
        ("state.json", "ENGINE_STATE_STOPPED"),
        ("state.json", "ENGINE_STATE_READY"),
        ("network.json", "configured_mode"),
        ("settings.json", "https_proxy_override"),
        ("versions.json", "daemon"),
    ] {
        assert!(entries[Path::new(name)].contains(expected), "{name}");
    }
    for contents in entries.values() {
        for secret in [
            "opaque_",
            "opaque-proxy-host",
            "AUTHORIZATION",
            "AWS_SECRET_ACCESS_KEY",
            "secret_url",
            "192.0.2.17",
        ] {
            assert!(!contents.contains(secret), "secret in bundle");
        }
    }
}

#[tokio::test]
async fn shares_validate_canonical_paths_and_share_settings_revisions() {
    let fixture = Fixture::new(Vec::new()).await;
    let mut shares = SharesClient::new(fixture.channel.clone());
    let mut settings = SettingsClient::new(fixture.channel.clone());

    let root = MarsaPaths::from_home(fixture.home.path()).unwrap();
    let project = fixture.home.path().join("project");
    let alias = fixture.home.path().join("project-alias");
    std::fs::create_dir(&project).unwrap();
    std::os::unix::fs::symlink(&project, &alias).unwrap();

    let snapshot = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let add = |id, host_path: &std::path::Path| AddShareRequest {
        request_id: format!("30000000-0000-4000-8000-{id:012}"),
        host_path: host_path.to_str().unwrap().to_string(),
        persist: true,
        ..Default::default()
    };
    for (id, path) in [(1, root.root()), (2, root.root().parent().unwrap())] {
        assert_eq!(
            shares.add(add(id, path)).await.unwrap_err().code(),
            Code::PermissionDenied
        );
    }
    let document = Settings::from_toml(&snapshot.settings_toml).unwrap();
    let mut unsafe_document = document.clone();
    unsafe_document.shares.push(root.root().to_owned());
    assert_eq!(
        settings
            .update(UpdateSettingsRequest {
                request_id: "40000000-0000-4000-8000-000000000001".into(),
                expected_revision: snapshot.revision,
                settings_toml: unsafe_document.to_toml().unwrap(),
            })
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

    let request = add(3, &alias);
    let share = shares.add(request.clone()).await.unwrap().into_inner();
    assert_eq!(
        share.host_path,
        project.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(shares.add(request).await.unwrap().into_inner(), share);
    assert_eq!(
        shares
            .list(ListSharesRequest {})
            .await
            .unwrap()
            .into_inner()
            .shares,
        vec![share.clone()]
    );

    let updated = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(updated.revision, snapshot.revision + 1);
    assert_eq!(
        Settings::from_toml(&updated.settings_toml).unwrap().shares,
        [project.canonicalize().unwrap()]
    );
    assert_eq!(
        std::fs::read_to_string(root.config_file()).unwrap(),
        updated.settings_toml
    );
    let mut stale = document;
    stale.vm.cpus += 1;
    assert_eq!(
        settings
            .update(UpdateSettingsRequest {
                request_id: "40000000-0000-4000-8000-000000000002".into(),
                expected_revision: snapshot.revision,
                settings_toml: stale.to_toml().unwrap(),
            })
            .await
            .unwrap_err()
            .code(),
        Code::Aborted
    );

    // Removal uses the stored ID, including when the directory no longer exists.
    std::fs::remove_dir(project).unwrap();
    let remove = RemoveShareRequest {
        request_id: "30000000-0000-4000-8000-000000000004".into(),
        share_id: share.id,
    };
    shares.remove(remove.clone()).await.unwrap();
    shares.remove(remove).await.unwrap();
    assert!(
        shares
            .list(ListSharesRequest {})
            .await
            .unwrap()
            .into_inner()
            .shares
            .is_empty()
    );
}

#[tokio::test]
async fn discovers_the_endpoint_without_registering_a_context() {
    use marsa_proto::host::v1::{
        GetDockerEndpointRequest, docker_discovery_client::DockerDiscoveryClient,
        docker_discovery_server::DockerDiscovery,
    };
    let fixture = Fixture::new(Vec::new()).await;
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    assert_eq!(
        fixture
            .api
            .get_endpoint(tonic::Request::new(GetDockerEndpointRequest {}))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let endpoint = DockerDiscoveryClient::new(fixture.channel.clone())
        .get_endpoint(GetDockerEndpointRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        endpoint.socket_path,
        paths.docker_socket().to_str().unwrap()
    );
    assert_eq!(
        endpoint.docker_host,
        format!("unix://{}", endpoint.socket_path)
    );
    assert_eq!(endpoint.context_name, "marsa");
    assert!(!fixture.home.path().join(".docker").exists());
}

struct Fixture {
    api: TypedApi,
    supervisor: Supervisor,
    channel: Channel,
    home: TempDir,
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
            home,
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

#[tokio::test]
async fn follows_classified_logs_across_append_and_rotation() {
    use marsa_proto::host::v1::{LogsRequest, diagnostics_client::DiagnosticsClient};
    use std::{
        fs,
        io::Write,
        os::unix::fs::{PermissionsExt, symlink},
    };

    let fixture = Fixture::new(vec![]).await;
    let paths = MarsaPaths::from_home(fixture.home.path()).unwrap();
    for path in [paths.root().to_owned(), paths.logs_dir(), paths.vm_dir()] {
        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => panic!("{error}"),
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let first = paths.logs_dir().join("marsad.2026-11-22.jsonl");
    let next = paths.logs_dir().join("marsad.2026-11-23.jsonl");
    const FIRST: &str = "{\"timestamp\":\"2026-11-22T00:00:00.123Z\",\"level\":\"INFO\",\"fields\":{\"message\":\"daemon started\",\"token\":\"opaque_daemon_secret\"}}\n";
    const SECOND: &str =
        "{\"level\":\"INFO\",\"fields\":{\"message\":\"daemon shutdown requested\"}}\n";
    const ROTATED: &str = "{\"level\":\"INFO\",\"fields\":{\"message\":\"daemon stopped\"}}\n";
    for (path, text) in [
        (first.as_path(), FIRST),
        (
            paths.console_log().as_path(),
            "[    0.00000] Kernel command line: opaque_guest_secret\n",
        ),
    ] {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    symlink(&first, paths.daemon_log()).unwrap();
    let mut client = DiagnosticsClient::new(fixture.channel.clone());
    let mut tail = client
        .logs(LogsRequest {
            follow: false,
            tail_lines: 2,
        })
        .await
        .unwrap()
        .into_inner();
    let daemon = next_log(&mut tail).await;
    assert_eq!(
        (daemon.component.as_str(), daemon.message.as_str()),
        ("daemon", "daemon_started")
    );
    assert_eq!(daemon.at_unix_millis, 1795305600123);
    let guest = next_log(&mut tail).await;
    assert_eq!(
        (guest.component.as_str(), guest.message.as_str()),
        ("guest", "kernel_command_line_redacted")
    );
    assert!(tail.message().await.unwrap().is_none());

    let mut follow = client
        .logs(LogsRequest {
            follow: true,
            tail_lines: 2,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(next_log(&mut follow).await, daemon);
    assert_eq!(next_log(&mut follow).await, guest);
    fs::OpenOptions::new()
        .append(true)
        .open(&first)
        .unwrap()
        .write_all(SECOND.as_bytes())
        .unwrap();
    assert_eq!(next_log(&mut follow).await.message, "shutdown_requested");
    fs::write(&next, ROTATED).unwrap();
    fs::set_permissions(&next, fs::Permissions::from_mode(0o600)).unwrap();
    let replacement = paths.logs_dir().join(".current-log");
    symlink(&next, &replacement).unwrap();
    fs::rename(&replacement, paths.daemon_log()).unwrap();
    assert_eq!(next_log(&mut follow).await.message, "daemon_stopped");
}

async fn next_log(
    stream: &mut Streaming<marsa_proto::host::v1::LogEntry>,
) -> marsa_proto::host::v1::LogEntry {
    timeout(WAIT, stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
#[path = "typed_api/settings_reset.rs"]
mod settings_reset;
