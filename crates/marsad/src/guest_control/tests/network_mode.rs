//! Connect the real packaged launch and control paths to a deterministic fake VM.

use std::{fs, os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

use hyper_util::rt::TokioIo;
use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode as ConfigMode, Settings},
};
use marsa_proto::{
    guest::{BIND_AUTHORIZATION_PORT, DOCKER_API_PORT},
    host::v1::{
        EngineActionRequest, EngineState, GetSettingsRequest, NetworkMode, NetworkStatusRequest,
        SetNetworkModeRequest, engine_client::EngineClient, network_client::NetworkClient,
        settings_client::SettingsClient,
    },
};
use marsa_vm::{Hypervisor, fake::FakeHypervisor};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::oneshot,
};
use tonic::transport::Endpoint;
use tower::service_fn;

use crate::{supervisor::SupervisorConfig, typed_api::TypedApi};

use super::fixtures::{client, command_result, docker_ping, expect, health, send, spec};
use super::{GuestResponse, WAIT, boot_proxy_script};
use marsa_proto::guest::{
    CONTROL_PORT,
    v1::{
        CommandResult, CommandStatus, ConfigureNetwork, EngineHealthState, NativeDns, StartEngine,
        Trim, TrimResult, guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::fake::{AgentStep, FakeScenario};

fn backend(mode: ConfigMode) -> Arc<FakeHypervisor> {
    let policy = ConfigureNetwork {
        dns_servers: vec!["192.0.2.53".into()],
        native_dns: (mode == ConfigMode::Nat).then(|| NativeDns {
            explicit_servers: true,
            ..Default::default()
        }),
        ..Default::default()
    };
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
        send(0, health(EngineHealthState::Ready)),
    ]);
    if mode == ConfigMode::Nat {
        script.extend([
            expect(4, HostBody::ConfigureNetwork(policy)),
            command_result(4, CommandStatus::Ok),
            expect(5, HostBody::Trim(Trim {})),
            send(5, GuestBody::TrimResult(TrimResult::default())),
        ]);
    }
    script.push(AgentStep::Wait(WAIT));
    Arc::new(FakeHypervisor::new(
        FakeScenario::default()
            .with_agent(CONTROL_PORT, script)
            .with_agent(DOCKER_API_PORT, docker_ping(200))
            .with_agent(BIND_AUTHORIZATION_PORT, vec![]),
    ))
}

#[tokio::test]
async fn packaged_restart_uses_the_new_pin_without_switching_the_running_vm() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let paths = MarsaPaths::from_home(home.path()).unwrap();
    fs::create_dir_all(paths.state_dir()).unwrap();
    fs::set_permissions(paths.state_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = paths.root().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut settings = Settings::default();
    settings.network.mode = ConfigMode::Nat;
    settings.network.dns = vec!["192.0.2.53".parse().unwrap()];
    let control = client();
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), settings)
        .unwrap()
        .with_packaged_runtime(
            backend(ConfigMode::Nat).capabilities(),
            control.clone(),
            SupervisorConfig {
                boot_timeout: Duration::from_secs(2),
                stop_timeout: Duration::from_millis(100),
                idle_timeout: Duration::from_secs(30),
                idle_stop: false,
                max_retries: 0,
                initial_backoff: Duration::ZERO,
                maximum_backoff: Duration::ZERO,
            },
            move |settings, _| Ok((backend(settings.network.mode), spec())),
        )
        .await
        .unwrap();
    let events_api = api.clone();
    let (shutdown, stop) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stop.await;
        })
        .await
        .unwrap();
    });
    let channel = Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_| {
            let socket = socket.clone();
            async move { UnixStream::connect(socket).await.map(TokioIo::new) }
        }))
        .await
        .unwrap();

    let mut engine = EngineClient::new(channel.clone());
    let action = |id| EngineActionRequest {
        request_id: format!("55000000-0000-4000-8000-{id:012}"),
    };
    let ready = engine.start(action(1)).await.unwrap().into_inner();
    assert_eq!(ready.state(), EngineState::Ready);
    let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
    events
        .send(crate::host_events::HostEvent::NetworkChanged.into())
        .unwrap();
    drop(events);
    events_api.watch_host_events(receiver).await;
    assert!(matches!(
        control.request(HostBody::Trim(Trim {})).await.unwrap(),
        GuestResponse::Trim(_)
    ));
    let mut net = NetworkClient::new(channel.clone());
    let changed = net
        .set_mode(SetNetworkModeRequest {
            request_id: action(2).request_id,
            mode: NetworkMode::Userspace.into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(changed.configured_mode(), NetworkMode::Userspace);
    assert_eq!(changed.active_mode(), NetworkMode::Nat);
    assert_eq!(changed.restart_pending, Some(true));
    let mut settings = SettingsClient::new(channel);
    assert!(
        settings
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner()
            .restart_required
    );

    let restarted = engine.restart(action(3)).await.unwrap().into_inner();
    assert_eq!(restarted.state(), EngineState::Ready);
    let actual = net
        .status(NetworkStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(actual.configured_mode(), NetworkMode::Userspace);
    assert_eq!(actual.active_mode(), NetworkMode::Userspace);
    assert_eq!(actual.restart_pending, Some(false));
    assert!(actual.reason.is_none());
    assert!(
        !settings
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner()
            .restart_required
    );
    drop(net);
    drop(settings);
    drop(engine);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}
