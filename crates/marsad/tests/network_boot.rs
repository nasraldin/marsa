//! Test automatic boot selection with the packaged ARM64 guest and the native VZ backend.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::{
    env,
    fs::{self, File},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use hyper_util::rt::TokioIo;
use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::host::v1::{
    EngineActionRequest, EngineState, SubscribeRequest, daemon_event::Body,
    engine_client::EngineClient, events_client::EventsClient,
};
use marsa_vm::{DiskSpec, NetworkSpec, VmSpec};
use marsa_vm_vz::VzHypervisor;
use marsad::{
    guest_control::GuestControl,
    supervisor::{NetworkResult, SupervisorConfig},
    typed_api::TypedApi,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::oneshot,
    time::timeout,
};
use tonic::transport::Endpoint;
use tower::service_fn;

#[tokio::test]
#[ignore = "requires the virtualization entitlement, packaged guest and network helper"]
async fn packaged_mode_selection_verifies_egress_before_engine_ready() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();
    let guest = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"))
        .canonicalize()
        .unwrap();
    let helper =
        PathBuf::from(env::var_os("MARSA_VZ_NETWORK_HELPER").expect("set MARSA_VZ_NETWORK_HELPER"));
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let data = scratch.path().join("data.img");
    File::create(&data)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    let console = scratch.path().join("console.log");
    let spec = VmSpec {
        cpus: 2,
        memory_bytes: 1024 * 1024 * 1024,
        kernel: guest.join("Image"),
        initramfs: None,
        cmdline: "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init panic=-1"
            .into(),
        root_disk: DiskSpec {
            path: guest.join("rootfs.img"),
            read_only: true,
        },
        data_disk: DiskSpec {
            path: data,
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Nat,
        console_log: console.clone(),
    };
    let paths = MarsaPaths::from_home(scratch.path()).unwrap();
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), Settings::default()).unwrap();
    let (api, supervisor) = api.with_engine(
        Arc::new(VzHypervisor::with_userspace(helper)),
        Arc::new(GuestControl::default()),
        spec,
        SupervisorConfig {
            boot_timeout: Duration::from_secs(60),
            stop_timeout: Duration::from_secs(6),
            idle_timeout: Duration::from_secs(60),
            idle_stop: false,
            max_retries: 0,
            initial_backoff: Duration::ZERO,
            maximum_backoff: Duration::ZERO,
        },
    );
    let socket = scratch.path().join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (shutdown, stop) = oneshot::channel();
    let serving = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stop.await;
        })
        .await
    });
    let channel = Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_| {
            let socket = socket.clone();
            async move { UnixStream::connect(socket).await.map(TokioIo::new) }
        }))
        .await
        .unwrap();
    let mut events = EventsClient::new(channel.clone())
        .subscribe(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut engine = EngineClient::new(channel.clone());
    let started = engine
        .start(EngineActionRequest {
            request_id: "22000000-0000-4000-8000-000000000001".into(),
        })
        .await;
    let transcript = fs::read_to_string(&console).unwrap_or_default();
    assert_eq!(
        started
            .unwrap_or_else(|error| panic!("{error}\n{transcript}"))
            .into_inner()
            .state(),
        EngineState::Ready
    );
    let network = supervisor
        .snapshot()
        .network
        .expect("ready guest has an attachment");
    let event = timeout(Duration::from_secs(5), async {
        while let Some(event) = events.message().await.unwrap() {
            if let Some(Body::Network(status)) = event.body {
                return status;
            }
        }
        panic!("the daemon did not publish its active attachment");
    })
    .await
    .unwrap();
    if matches!(network.result, NetworkResult::Fallback(_)) {
        assert_eq!(
            event.active_mode(),
            marsa_proto::host::v1::NetworkMode::Userspace
        );
        assert!(
            event
                .reason
                .as_deref()
                .unwrap()
                .ends_with("_userspace_verified")
        );
    }
    drop(events);
    engine
        .stop(EngineActionRequest {
            request_id: "22000000-0000-4000-8000-000000000002".into(),
        })
        .await
        .unwrap();
    drop(engine);
    drop(channel);
    shutdown.send(()).unwrap();
    serving.await.unwrap().unwrap();
    eprintln!("Verified boot network: {network:?}");
    if env::var_os("MARSA_EXPECT_NATIVE_FALLBACK").is_some() {
        assert_eq!(
            network.active,
            NetworkSpec::Userspace,
            "{network:?}\n{transcript}"
        );
        assert!(
            matches!(network.result, NetworkResult::Fallback(_)),
            "{network:?}"
        );
    } else {
        assert!(
            matches!(
                (network.active, network.result),
                (NetworkSpec::Nat, NetworkResult::NativeHealthy)
                    | (NetworkSpec::Userspace, NetworkResult::Fallback(_))
            ),
            "{network:?}"
        );
    }
}
