//! Observe a real host sleep and wake with an entitled, offline guest.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::{
    env,
    fs::File,
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use hyper_util::rt::TokioIo;
use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, Settings},
};
use marsa_proto::{
    guest::DOCKER_API_PORT,
    host::v1::{
        EngineState, HostEventKind, SubscribeRequest, daemon_event::Body,
        events_client::EventsClient,
    },
};
use marsa_vm::{DiskSpec, Hypervisor, NetworkSpec, Vm, VmSpec, async_trait};
use marsa_vm_vz::VzHypervisor;
use marsad::{
    guest_control::GuestControl,
    host_events::platform::Watcher,
    supervisor::{GuestProbe, GuestSession, SupervisorConfig},
    typed_api::TypedApi,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot},
    time::timeout,
};
use tonic::transport::Endpoint;
use tower::service_fn;

const CLOCK_TIMEOUT: Duration = Duration::from_secs(3);
const RESPONSE_LIMIT: u64 = 64 * 1024;

#[tokio::test]
#[ignore = "requires the virtualization entitlement, packaged guest and manually operated Mac"]
async fn real_host_sleep_resumes_and_synchronizes_guest_clock() {
    let guest = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"))
        .canonicalize()
        .unwrap();
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let data = scratch.path().join("data.img");
    File::create(&data)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    let console = scratch.path().join("console.log");
    let mut settings = Settings::default();
    let (backend, cmdline): (Arc<dyn Hypervisor>, &str) = match env::var_os(
        "MARSA_VZ_NETWORK_HELPER",
    ) {
        Some(helper) => {
            settings.network.mode = NetworkMode::Userspace;
            (
                Arc::new(VzHypervisor::with_userspace(helper.into())),
                "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init panic=-1",
            )
        }
        None => {
            settings.network.mode = NetworkMode::Nat;
            (
                Arc::new(VzHypervisor),
                "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init marsa.interface=none panic=-1",
            )
        }
    };
    let spec = VmSpec {
        cpus: 2,
        memory_bytes: 1024 * 1024 * 1024,
        kernel: guest.join("Image"),
        initramfs: None,
        cmdline: cmdline.into(),
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
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), settings).unwrap();
    let (send_sample, mut samples) = mpsc::unbounded_channel();
    let (api, supervisor) = api.with_engine(
        backend,
        Arc::new(MeasureClock {
            control: GuestControl::default(),
            samples: send_sample,
        }),
        spec,
        SupervisorConfig {
            boot_timeout: Duration::from_secs(60),
            stop_timeout: Duration::from_secs(6),
            idle_timeout: Duration::from_secs(300),
            idle_stop: false,
            max_retries: 0,
            initial_backoff: Duration::ZERO,
            maximum_backoff: Duration::ZERO,
        },
    );

    let (watcher, source) = Watcher::start().unwrap();
    let observing = {
        let api = api.clone();
        tokio::spawn(async move { api.watch_host_events(source).await })
    };
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

    supervisor.ensure_ready().await.unwrap_or_else(|error| {
        panic!(
            "{error}\n{}",
            std::fs::read_to_string(console).unwrap_or_default()
        )
    });
    samples
        .recv()
        .await
        .unwrap()
        .assert_within_one_second("before sleep");
    let mut events = EventsClient::new(channel)
        .subscribe(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();

    if env::var_os("MARSA_POWER_WATCHER_PREFLIGHT").is_none() {
        assert!(matches!(
            events.message().await.unwrap().unwrap().body,
            Some(Body::Snapshot(snapshot)) if snapshot.engine.as_ref().unwrap().state() == EngineState::Ready
        ));
        eprintln!(
            "M3-11 ready for manual system sleep. Sleep the Mac and wake it within two minutes."
        );
        timeout(Duration::from_secs(180), async {
            for expected in [
                Observation::Host(HostEventKind::WillSleep),
                Observation::Engine(EngineState::Paused),
                Observation::Host(HostEventKind::Woke),
                Observation::Engine(EngineState::Ready),
            ] {
                loop {
                    let event = events.message().await.unwrap().unwrap();
                    let observed = match event.body {
                        Some(Body::Host(host)) if host.kind() == HostEventKind::NetworkChanged => {
                            continue;
                        }
                        Some(Body::Host(host)) => Observation::Host(host.kind()),
                        Some(Body::Engine(engine)) => Observation::Engine(engine.state()),
                        _ => continue,
                    };
                    assert_eq!(observed, expected, "event {}", event.sequence);
                    eprintln!("M3-11 event {}: {observed:?}", event.sequence);
                    break;
                }
            }
        })
        .await
        .expect("real host sleep/wake sequence did not finish");
        samples
            .recv()
            .await
            .unwrap()
            .assert_within_one_second("after wake");
    }

    drop(events);
    drop(watcher);
    observing.await.unwrap();
    shutdown.send(()).unwrap();
    serving.await.unwrap().unwrap();
}

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Host(HostEventKind),
    Engine(EngineState),
}

struct MeasureClock {
    control: GuestControl,
    samples: mpsc::UnboundedSender<ClockSample>,
}

struct SamplingSession {
    guest: Box<dyn GuestSession>,
    samples: mpsc::UnboundedSender<ClockSample>,
}

#[async_trait]
impl GuestProbe for MeasureClock {
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        Ok(Box::new(SamplingSession {
            guest: self.control.handshake(vm).await?,
            samples: self.samples.clone(),
        }))
    }
}

#[async_trait]
impl GuestSession for SamplingSession {
    async fn engine_ready(&mut self, vm: &dyn Vm) -> io::Result<()> {
        self.guest.engine_ready(vm).await?;
        let sample = timeout(CLOCK_TIMEOUT, read_guest_clock(vm)).await??;
        self.samples.send(sample).map_err(io::Error::other)
    }

    async fn synchronize_clock(&mut self) -> io::Result<()> {
        self.guest.synchronize_clock().await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.guest.shutdown().await
    }

    async fn disconnected(&mut self) -> io::Error {
        self.guest.disconnected().await
    }
}

struct ClockSample {
    before: OffsetDateTime,
    guest: OffsetDateTime,
    after: OffsetDateTime,
}

impl ClockSample {
    fn assert_within_one_second(&self, phase: &str) {
        let bound = (self.before - self.guest)
            .abs()
            .max((self.after - self.guest).abs());
        eprintln!(
            "M3-11 clock {phase}: host_before={}, guest={}, host_after={}, maximum_skew={}µs",
            self.before.unix_timestamp_nanos(),
            self.guest.unix_timestamp_nanos(),
            self.after.unix_timestamp_nanos(),
            bound.whole_microseconds()
        );
        assert!(self.after >= self.before, "host clock moved backward");
        assert!(
            self.after - self.before <= time::Duration::SECOND,
            "clock measurement exceeded one second"
        );
        assert!(
            bound <= time::Duration::SECOND,
            "clock skew exceeded one second"
        );
    }
}

async fn read_guest_clock(vm: &dyn Vm) -> io::Result<ClockSample> {
    let mut vsock = vm
        .connect_vsock(DOCKER_API_PORT)
        .await
        .map_err(io::Error::other)?;
    let before = SystemTime::now().into();
    vsock
        .write_all(b"GET /info HTTP/1.0\r\nHost: docker\r\nConnection: close\r\n\r\n")
        .await?;
    let mut response = Vec::new();
    vsock
        .take(RESPONSE_LIMIT)
        .read_to_end(&mut response)
        .await?;
    let after = SystemTime::now().into();
    if response.len() as u64 == RESPONSE_LIMIT || !response.starts_with(b"HTTP/1.0 200 ") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Docker info response",
        ));
    }
    let payload = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .map(|index| &response[index + 4..])
        .ok_or(io::ErrorKind::InvalidData)?;
    let info: serde_json::Value = serde_json::from_slice(payload)?;
    let time = info["SystemTime"]
        .as_str()
        .ok_or(io::ErrorKind::InvalidData)?;
    let guest = OffsetDateTime::parse(time, &Rfc3339).map_err(io::Error::other)?;
    Ok(ClockSample {
        before,
        guest,
        after,
    })
}
