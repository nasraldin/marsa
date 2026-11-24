//! Observe and kill private daemon fixtures at each supervisor boundary.

use std::{env, fs, io, os::unix::process::ExitStatusExt, path::Path, process::Stdio, sync::Arc};

use hyper_util::rt::TokioIo;
use marsa_proto::host::v1::{EngineState, EngineStatusRequest, engine_client::EngineClient};
use marsa_vm::{
    DiskSpec, NetworkSpec, Vm, VmSpec, async_trait,
    fake::{FakeHypervisor, FakeScenario},
};
use marsad::supervisor::{GuestProbe, GuestSession, Supervisor, SupervisorConfig};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tokio::{
    net::{UnixListener, UnixStream},
    process::{Child, Command},
    time::sleep,
};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

use super::*;

const SCENARIO: &str = "MARSA_TEST_CRASH_STATE";
const LIMIT: Duration = Duration::from_secs(5);
const HOLD: Duration = Duration::from_secs(30);

#[tokio::test]
async fn restart_recovers_every_supervisor_state() {
    for state in [
        EngineState::Stopped,
        EngineState::Starting,
        EngineState::Booting,
        EngineState::Ready,
        EngineState::Idle,
        EngineState::Paused,
        EngineState::Stopping,
        EngineState::Failed,
    ] {
        let home = tempfile::tempdir_in("/tmp").unwrap();
        let paths = MarsaPaths::from_home(home.path()).unwrap();
        let mut first = launch(home.path(), state);
        at_state(&mut first, &paths, state).await;
        let first_pid = first.id().unwrap();
        first.start_kill().unwrap();
        assert_eq!(first.wait().await.unwrap().signal(), Some(9), "{state:?}");
        assert!(paths.daemon_socket().exists());
        assert!(paths.docker_socket().exists());
        assert!(paths.agent_socket().exists());
        assert_eq!(
            fs::read_to_string(paths.daemon_pid_file()).unwrap().trim(),
            first_pid.to_string()
        );

        let mut replacement = launch(home.path(), EngineState::Ready);
        at_state(&mut replacement, &paths, EngineState::Ready).await;
        let replacement_pid = replacement.id().unwrap();
        assert_eq!(
            fs::read_to_string(paths.daemon_pid_file()).unwrap().trim(),
            replacement_pid.to_string()
        );
        drop(UnixStream::connect(paths.docker_socket()).await.unwrap());
        drop(UnixStream::connect(paths.agent_socket()).await.unwrap());

        kill(
            Pid::from_raw(replacement_pid.try_into().unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        assert!(
            timeout(LIMIT, replacement.wait())
                .await
                .unwrap()
                .unwrap()
                .success(),
            "{state:?}"
        );
        assert!(!paths.daemon_socket().exists());
        assert!(!paths.docker_socket().exists());
        assert!(!paths.agent_socket().exists());
        assert!(!paths.daemon_pid_file().exists());
    }
}

fn launch(home: &Path, state: EngineState) -> Child {
    Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "platform::crash_tests::crash_fixture",
            "--nocapture",
        ])
        .env("HOME", home)
        .env(SCENARIO, state.as_str_name())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn at_state(child: &mut Child, paths: &MarsaPaths, expected: EngineState) {
    timeout(LIMIT, async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "fixture exited before {expected:?}"
            );
            if let Ok(channel) = connect(paths).await
                && let Ok(status) = EngineClient::new(channel)
                    .status(EngineStatusRequest {})
                    .await
                && status.into_inner().state() == expected
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fixture never reached {expected:?}"));
}

async fn connect(paths: &MarsaPaths) -> Result<Channel, tonic::transport::Error> {
    let socket = paths.daemon_socket();
    Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_| {
            let socket = socket.clone();
            async move { UnixStream::connect(socket).await.map(TokioIo::new) }
        }))
        .await
}

#[tokio::test]
async fn crash_fixture() {
    let Ok(name) = env::var(SCENARIO) else { return };
    let target = EngineState::from_str_name(&name).unwrap();
    let paths = MarsaPaths::from_home(env::home_dir().unwrap()).unwrap();
    let uid = require_unprivileged().unwrap().as_raw();
    let mut instance = files::Instance::acquire(&paths, uid).unwrap();
    let settings = files::read_settings(&paths, uid).unwrap();

    let scenario = match target {
        EngineState::Starting => FakeScenario::default().with_start_publication_delay(HOLD),
        EngineState::Failed => {
            FakeScenario::default().with_start_timeout(Duration::from_millis(10))
        }
        _ => FakeScenario::default(),
    };
    let (api, supervisor) = TypedApi::new(paths.clone(), uid, settings)
        .unwrap()
        .with_engine(
            Arc::new(FakeHypervisor::new(scenario)),
            Arc::new(Probe(target)),
            VmSpec {
                cpus: 2,
                memory_bytes: 512 * 1024 * 1024,
                kernel: paths.vm_dir().join("Image"),
                initramfs: None,
                cmdline: String::new(),
                root_disk: DiskSpec {
                    path: paths.vm_dir().join("rootfs.img"),
                    read_only: true,
                },
                data_disk: DiskSpec {
                    path: paths.data_disk(),
                    read_only: false,
                },
                shares: Vec::new(),
                network: NetworkSpec::Nat,
                console_log: paths.console_log(),
            },
            SupervisorConfig {
                boot_timeout: HOLD,
                stop_timeout: HOLD,
                idle_timeout: Duration::from_millis(20),
                idle_stop: false,
                max_retries: 1,
                initial_backoff: HOLD,
                maximum_backoff: HOLD,
            },
        );
    let (socket, docker, agent) = instance.publish(&paths, uid).unwrap();
    let server = serve(
        UnixListener::from_std(socket).unwrap(),
        UnixListener::from_std(docker).unwrap(),
        UnixListener::from_std(agent).unwrap(),
        api,
        uid,
        Signals::register().unwrap(),
    );
    let (result, ()) = tokio::join!(server, drive(supervisor, target));
    result.unwrap();
    instance.cleanup().unwrap();
}

async fn drive(supervisor: Supervisor, target: EngineState) {
    if target == EngineState::Stopped {
        return;
    }
    let ready = supervisor.ensure_ready().await;
    if matches!(
        target,
        EngineState::Starting | EngineState::Booting | EngineState::Failed
    ) {
        return;
    }
    ready.unwrap();
    match target {
        EngineState::Idle => supervisor.report_containers(0).await.unwrap(),
        EngineState::Paused => supervisor.pause().await.unwrap(),
        EngineState::Stopping => supervisor.stop().await.unwrap(),
        _ => (),
    }
}

struct Probe(EngineState);

#[async_trait]
impl GuestProbe for Probe {
    async fn handshake(&self, _: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        if self.0 == EngineState::Booting {
            return std::future::pending().await;
        }
        Ok(Box::new(Session(self.0)))
    }
}

struct Session(EngineState);

#[async_trait]
impl GuestSession for Session {
    async fn engine_ready(&mut self, _: &dyn Vm) -> io::Result<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        if self.0 == EngineState::Stopping {
            return std::future::pending().await;
        }
        Err(io::ErrorKind::Unsupported.into())
    }
}
