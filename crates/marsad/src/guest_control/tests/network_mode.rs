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

use super::fixtures::{client, docker_ping, normal_session, spec};

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
    let backend = Arc::new(FakeHypervisor::new(
        normal_session()
            .with_agent(DOCKER_API_PORT, docker_ping(200))
            .with_agent(BIND_AUTHORIZATION_PORT, vec![]),
    ));
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), settings)
        .unwrap()
        .with_packaged_runtime(
            backend.capabilities(),
            client(),
            SupervisorConfig {
                boot_timeout: Duration::from_secs(2),
                stop_timeout: Duration::from_millis(100),
                idle_timeout: Duration::from_secs(30),
                idle_stop: false,
                max_retries: 0,
                initial_backoff: Duration::ZERO,
                maximum_backoff: Duration::ZERO,
            },
            move |_, _| Ok((backend.clone(), spec())),
        )
        .await
        .unwrap();
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
