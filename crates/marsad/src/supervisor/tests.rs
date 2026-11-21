//! Verify each supervisor transition with a portable, scripted guest.

use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use marsa_vm::{
    Capabilities, DiskSpec, Hypervisor, NetworkSpec, Vm, VmError, VmSpec, VsockStream, async_trait,
    fake::{AgentStep, FakeHypervisor, FakeScenario},
};
use tokio::{
    io::AsyncReadExt,
    sync::broadcast,
    time::{advance, timeout},
};

use super::{
    BootStage, GuestProbe, GuestSession, State, Supervisor, SupervisorConfig, SupervisorError,
    Transition,
};

mod network;

const PORT: u32 = 5000;
const STEP: Duration = Duration::from_millis(20);

struct ScriptProbe;

#[async_trait]
impl GuestProbe for ScriptProbe {
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        let mut stream = vm.connect_vsock(PORT).await.map_err(io::Error::other)?;
        read_marker(&mut stream, b'H').await?;
        Ok(Box::new(ScriptSession(stream)))
    }
}

struct ScriptSession(VsockStream);

#[async_trait]
impl GuestSession for ScriptSession {
    async fn engine_ready(&mut self, _: &dyn Vm) -> io::Result<()> {
        read_marker(&mut self.0, b'R').await
    }
}

async fn read_marker(stream: &mut VsockStream, expected: u8) -> io::Result<()> {
    if stream.read_u8().await? != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected protocol marker",
        ));
    }
    Ok(())
}

struct CountingHypervisor {
    normal: FakeHypervisor,
    initial: Option<FakeHypervisor>,
    created: Arc<AtomicUsize>,
}

#[async_trait]
impl Hypervisor for CountingHypervisor {
    fn capabilities(&self) -> Capabilities {
        self.normal.capabilities()
    }

    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError> {
        if self.created.fetch_add(1, Ordering::Relaxed) == 0
            && let Some(initial) = &self.initial
        {
            return initial.create(spec).await;
        }
        self.normal.create(spec).await
    }
}

fn create(
    config: SupervisorConfig,
    console: PathBuf,
    normal: FakeScenario,
    initial: Option<FakeScenario>,
) -> (Supervisor, Arc<AtomicUsize>) {
    let created = Arc::new(AtomicUsize::new(0));
    let backend = CountingHypervisor {
        normal: FakeHypervisor::new(normal),
        initial: initial.map(FakeHypervisor::new),
        created: created.clone(),
    };
    let spec = VmSpec {
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
        console_log: console,
    };
    (
        Supervisor::spawn(Arc::new(backend), Arc::new(ScriptProbe), spec, config),
        created,
    )
}

fn config() -> SupervisorConfig {
    SupervisorConfig {
        boot_timeout: STEP * 10,
        stop_timeout: STEP * 3,
        idle_timeout: STEP * 3,
        idle_stop: false,
        max_retries: 0,
        initial_backoff: STEP,
        maximum_backoff: STEP * 4,
    }
}

fn ready() -> FakeScenario {
    FakeScenario::default().with_agent(PORT, vec![AgentStep::Send(b"HR".to_vec())])
}

async fn transition(
    stream: &mut broadcast::Receiver<Transition>,
    previous: State,
    current: State,
) -> Transition {
    let event = timeout(STEP * 50, stream.recv()).await.unwrap().unwrap();
    assert_eq!((event.previous, event.current), (previous, current));
    event
}

#[tokio::test(start_paused = true)]
async fn one_boot_resolves_concurrent_waiters_only_after_both_checks() {
    let delayed = FakeScenario::default().with_agent(
        PORT,
        vec![
            AgentStep::Send(b"H".to_vec()),
            AgentStep::Wait(STEP * 3),
            AgentStep::Send(b"R".to_vec()),
        ],
    );
    let (supervisor, created) = create(config(), "/fixture/console.log".into(), delayed, None);
    let mut stream = supervisor.events();
    let first = supervisor.clone();
    let first = tokio::spawn(async move { first.ensure_ready().await });
    transition(&mut stream, State::Stopped, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    let second = supervisor.clone();
    let second = tokio::spawn(async move { second.ensure_ready().await });
    assert!(!first.is_finished() && !second.is_finished());
    transition(&mut stream, State::Booting, State::Ready).await;
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 1);
    supervisor.stop().await.unwrap();
    transition(&mut stream, State::Ready, State::Stopping).await;
    transition(&mut stream, State::Stopping, State::Stopped).await;
}

#[tokio::test(start_paused = true)]
async fn timeout_captures_the_stage_and_sanitized_console_for_all_waiters() {
    let dir = tempfile::tempdir().unwrap();
    let console = dir.path().canonicalize().unwrap().join("console.log");
    std::fs::write(&console, b"[guest] waiting\x1b setup\n").unwrap();
    let stalled = FakeScenario::default().with_agent(PORT, vec![AgentStep::Wait(STEP * 50)]);
    let (supervisor, created) = create(config(), console, stalled, None);
    let mut stream = supervisor.events();
    let (first, second) = tokio::join!(supervisor.ensure_ready(), supervisor.ensure_ready());
    for result in [first, second] {
        let Err(SupervisorError::Boot(failure)) = result else {
            panic!("expected timeout")
        };
        assert_eq!(failure.stage, BootStage::Handshake);
        assert!(failure.timed_out);
        assert_eq!(failure.console_tail, "[guest] waiting setup\n");
    }
    transition(&mut stream, State::Stopped, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    transition(&mut stream, State::Booting, State::Failed).await;
    transition(&mut stream, State::Failed, State::Stopped).await;
    assert_eq!(created.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn retries_recover_unexpected_exit_and_stop_at_the_configured_limit() {
    let dying = ready().with_exit_after(STEP * 3);
    let mut retry = config();
    retry.max_retries = 2;
    let (supervisor, created) = create(
        retry.clone(),
        "/fixture/console.log".into(),
        ready(),
        Some(dying),
    );
    let mut stream = supervisor.events();
    supervisor.ensure_ready().await.unwrap();
    transition(&mut stream, State::Stopped, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    transition(&mut stream, State::Booting, State::Ready).await;
    transition(&mut stream, State::Ready, State::Failed).await;
    transition(&mut stream, State::Failed, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    transition(&mut stream, State::Booting, State::Ready).await;
    assert_eq!(created.load(Ordering::Relaxed), 2);

    let stalled = FakeScenario::default().with_start_timeout(STEP);
    let (supervisor, created) = create(retry, "/fixture/console.log".into(), stalled, None);
    assert!(matches!(
        supervisor.ensure_ready().await,
        Err(SupervisorError::Boot(_))
    ));
    advance(STEP * 100).await;
    assert_eq!(created.load(Ordering::Relaxed), 3);
    assert_eq!(supervisor.snapshot().state, State::Stopped);
}

#[tokio::test(start_paused = true)]
async fn idle_request_and_pause_resume_reenter_ready() {
    let (supervisor, _) = create(config(), "/fixture/console.log".into(), ready(), None);
    supervisor.ensure_ready().await.unwrap();
    let mut stream = supervisor.events();
    supervisor.report_containers(0).await.unwrap();
    transition(&mut stream, State::Ready, State::Idle).await;
    supervisor.ensure_ready().await.unwrap();
    transition(&mut stream, State::Idle, State::Ready).await;
    supervisor.pause().await.unwrap();
    transition(&mut stream, State::Ready, State::Paused).await;
    supervisor.resume().await.unwrap();
    transition(&mut stream, State::Paused, State::Ready).await;
}

#[tokio::test(start_paused = true)]
async fn idle_stop_and_stop_during_boot_clean_up_the_guest() {
    let mut idle = config();
    idle.idle_stop = true;
    let (supervisor, _) = create(idle, "/fixture/console.log".into(), ready(), None);
    supervisor.ensure_ready().await.unwrap();
    let mut stream = supervisor.events();
    supervisor.report_containers(0).await.unwrap();
    transition(&mut stream, State::Ready, State::Idle).await;
    transition(&mut stream, State::Idle, State::Stopping).await;
    transition(&mut stream, State::Stopping, State::Stopped).await;

    let stalled = FakeScenario::default().with_agent(PORT, vec![AgentStep::Wait(STEP * 50)]);
    let (supervisor, _) = create(config(), "/fixture/console.log".into(), stalled, None);
    let mut stream = supervisor.events();
    let boot = supervisor.clone();
    let boot = tokio::spawn(async move { boot.ensure_ready().await });
    transition(&mut stream, State::Stopped, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    supervisor.stop().await.unwrap();
    assert!(matches!(boot.await.unwrap(), Err(SupervisorError::Stopped)));
    transition(&mut stream, State::Booting, State::Stopping).await;
    transition(&mut stream, State::Stopping, State::Stopped).await;
}

#[tokio::test(start_paused = true)]
async fn health_timeout_after_handshake_is_reported_as_engine_failure() {
    let stalled = FakeScenario::default().with_agent(
        PORT,
        vec![AgentStep::Send(b"H".to_vec()), AgentStep::Wait(STEP * 50)],
    );
    let (supervisor, _) = create(config(), "/fixture/console.log".into(), stalled, None);
    let Err(SupervisorError::Boot(failure)) = supervisor.ensure_ready().await else {
        panic!("expected health deadline");
    };
    assert_eq!(failure.stage, BootStage::Engine);
    assert!(failure.timed_out);
}

#[tokio::test(start_paused = true)]
async fn failed_shutdown_keeps_the_same_vm_until_cleanup_succeeds() {
    let (supervisor, created) = create(
        config(),
        "/fixture/console.log".into(),
        ready().with_failed_stops(1),
        None,
    );
    supervisor.ensure_ready().await.unwrap();
    assert!(matches!(
        supervisor.stop().await,
        Err(SupervisorError::Vm(_))
    ));
    assert_eq!(supervisor.snapshot().state, State::Failed);
    let Err(SupervisorError::Boot(failure)) = supervisor.ensure_ready().await else {
        panic!("expected pending shutdown failure");
    };
    assert_eq!(failure.stage, BootStage::Shutdown);
    assert_eq!(created.load(Ordering::Relaxed), 1);

    supervisor.stop().await.unwrap();
    supervisor.ensure_ready().await.unwrap();
    assert_eq!(created.load(Ordering::Relaxed), 2);
}

#[tokio::test(start_paused = true)]
async fn stop_during_backoff_cancels_retries() {
    let mut retry = config();
    retry.max_retries = 3;
    retry.initial_backoff = STEP * 20;
    retry.maximum_backoff = STEP * 30;
    let stalled = FakeScenario::default().with_start_timeout(STEP);
    let (supervisor, created) = create(retry, "/fixture/console.log".into(), stalled, None);
    let mut stream = supervisor.events();
    let boot = supervisor.clone();
    let boot = tokio::spawn(async move { boot.ensure_ready().await });
    transition(&mut stream, State::Stopped, State::Starting).await;
    transition(&mut stream, State::Starting, State::Failed).await;
    supervisor.stop().await.unwrap();
    transition(&mut stream, State::Failed, State::Stopping).await;
    transition(&mut stream, State::Stopping, State::Stopped).await;
    assert!(matches!(boot.await.unwrap(), Err(SupervisorError::Stopped)));
    advance(STEP * 100).await;
    assert_eq!(created.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn repeatedly_crashing_ready_guests_share_one_retry_budget() {
    let mut retry = config();
    retry.max_retries = 1;
    let dying = ready().with_exit_after(STEP * 3);
    let (supervisor, created) = create(retry, "/fixture/console.log".into(), dying, None);
    let mut stream = supervisor.events();
    supervisor.ensure_ready().await.unwrap();
    transition(&mut stream, State::Stopped, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    transition(&mut stream, State::Booting, State::Ready).await;
    transition(&mut stream, State::Ready, State::Failed).await;
    transition(&mut stream, State::Failed, State::Starting).await;
    transition(&mut stream, State::Starting, State::Booting).await;
    transition(&mut stream, State::Booting, State::Ready).await;
    transition(&mut stream, State::Ready, State::Failed).await;
    transition(&mut stream, State::Failed, State::Stopped).await;
    assert_eq!(created.load(Ordering::Relaxed), 2);
}
