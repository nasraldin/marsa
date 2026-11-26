//! Verify boot selection without external networks or a virtualization entitlement.

use std::{collections::VecDeque, net::Ipv4Addr, sync::Mutex};

use marsa_core::settings::NetworkMode;
use marsa_proto::guest::v1::EgressStage;

use super::*;
use crate::supervisor::{
    AlternateFailure, BootNetwork, NetworkResult,
    host_egress::Target,
    network::{HostCheck, Selection},
};

const TARGET: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 42);

struct AvailableHost;

#[async_trait]
impl HostCheck for AvailableHost {
    async fn verified_target(&self) -> Option<Target> {
        Some(Target::fixture(TARGET))
    }
}

struct OfflineHost;

#[async_trait]
impl HostCheck for OfflineHost {
    async fn verified_target(&self) -> Option<Target> {
        None
    }
}

type Trace = Arc<Mutex<Vec<&'static str>>>;

struct Backend {
    trace: Trace,
    scenario: FakeScenario,
    userspace: Option<FakeScenario>,
}

#[async_trait]
impl Hypervisor for Backend {
    fn capabilities(&self) -> Capabilities {
        FakeHypervisor::default().capabilities()
    }

    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError> {
        self.trace.lock().unwrap().push(match spec.network {
            NetworkSpec::Nat => "native",
            NetworkSpec::Userspace => "userspace",
        });
        let scenario = match spec.network {
            NetworkSpec::Userspace => self.userspace.as_ref().unwrap_or(&self.scenario),
            NetworkSpec::Nat => &self.scenario,
        };
        FakeHypervisor::new(scenario.clone()).create(spec).await
    }
}

struct Probe {
    trace: Trace,
    attempts: Mutex<VecDeque<Option<Vec<EgressStage>>>>,
    workloads: Option<u32>,
}

#[async_trait]
impl GuestProbe for Probe {
    async fn active_containers(&self, _: &dyn Vm) -> Option<io::Result<u32>> {
        self.workloads.map(Ok)
    }

    async fn handshake(&self, _: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        Ok(Box::new(Session {
            trace: self.trace.clone(),
            stages: self
                .attempts
                .lock()
                .unwrap()
                .pop_front()
                .unwrap()
                .map(|stages| Arc::new(Mutex::new(stages))),
        }))
    }
}

#[derive(Clone)]
struct Session {
    trace: Trace,
    stages: Option<Arc<Mutex<Vec<EgressStage>>>>,
}

#[async_trait]
impl super::super::LiveEgressProbe for Session {
    async fn probe(&mut self, ip: Ipv4Addr) -> io::Result<EgressStage> {
        self.probe_egress(ip).await
    }
}

#[async_trait]
impl GuestSession for Session {
    fn live_egress_probe(&self) -> Option<Box<dyn super::super::LiveEgressProbe>> {
        self.stages.as_ref().map(|_| Box::new(self.clone()) as _)
    }

    fn can_probe_egress(&self) -> bool {
        self.stages.is_some()
    }

    async fn probe_egress(&mut self, ip: Ipv4Addr) -> io::Result<EgressStage> {
        assert_eq!(ip, TARGET);
        self.trace.lock().unwrap().push("probe");
        Ok(self.stages.as_ref().unwrap().lock().unwrap().remove(0))
    }

    async fn engine_ready(&mut self, _: &dyn Vm) -> io::Result<()> {
        self.trace.lock().unwrap().push("engine");
        tokio::time::sleep(STEP * 2).await;
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.trace.lock().unwrap().push("retire");
        Err(io::ErrorKind::Unsupported.into())
    }
}

async fn boot(
    host: Option<Arc<dyn HostCheck>>,
    first_mode: NetworkSpec,
    scenario: FakeScenario,
    attempts: Vec<Option<Vec<EgressStage>>>,
) -> (
    Result<(), SupervisorError>,
    Option<BootNetwork>,
    Vec<&'static str>,
) {
    boot_with_candidate(host, first_mode, scenario, None, attempts).await
}

async fn boot_with_candidate(
    host: Option<Arc<dyn HostCheck>>,
    first_mode: NetworkSpec,
    scenario: FakeScenario,
    userspace: Option<FakeScenario>,
    attempts: Vec<Option<Vec<EgressStage>>>,
) -> (
    Result<(), SupervisorError>,
    Option<BootNetwork>,
    Vec<&'static str>,
) {
    let (supervisor, trace) = fixture(host, first_mode, scenario, userspace, attempts);
    let result = supervisor.ensure_ready().await;
    let network = supervisor.snapshot().network;
    let steps = trace.lock().unwrap().clone();
    (result, network, steps)
}

fn fixture(
    host: Option<Arc<dyn HostCheck>>,
    first_mode: NetworkSpec,
    scenario: FakeScenario,
    userspace: Option<FakeScenario>,
    attempts: Vec<Option<Vec<EgressStage>>>,
) -> (Supervisor, Trace) {
    fixture_workloads(host, first_mode, scenario, userspace, attempts, None)
}

fn fixture_workloads(
    host: Option<Arc<dyn HostCheck>>,
    first_mode: NetworkSpec,
    scenario: FakeScenario,
    userspace: Option<FakeScenario>,
    attempts: Vec<Option<Vec<EgressStage>>>,
    workloads: Option<u32>,
) -> (Supervisor, Trace) {
    fixture_mode(
        host,
        first_mode,
        scenario,
        userspace,
        attempts,
        workloads,
        NetworkMode::Auto,
    )
}

fn fixture_mode(
    host: Option<Arc<dyn HostCheck>>,
    first_mode: NetworkSpec,
    scenario: FakeScenario,
    userspace: Option<FakeScenario>,
    attempts: Vec<Option<Vec<EgressStage>>>,
    workloads: Option<u32>,
    configured_mode: NetworkMode,
) -> (Supervisor, Trace) {
    let trace = Trace::default();
    let backend = Arc::new(Backend {
        trace: trace.clone(),
        scenario,
        userspace,
    });
    let probe = Arc::new(Probe {
        trace: trace.clone(),
        attempts: Mutex::new(attempts.into()),
        workloads,
    });
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
        network: first_mode,
        console_log: "/fixture/console.log".into(),
    };
    let supervisor = Supervisor::spawn_inner(
        backend,
        probe,
        spec,
        config(),
        None,
        Selection {
            host,
            configured_mode: Some(configured_mode),
        },
    );
    (supervisor, trace)
}

#[tokio::test(start_paused = true)]
async fn qualifies_userspace_before_releasing_the_engine() {
    let (supervisor, trace) = fixture(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        None,
        vec![
            Some(vec![EgressStage::Route; 2]),
            Some(vec![EgressStage::Healthy]),
        ],
    );
    let stopped = supervisor
        .observe_network(NetworkMode::Userspace)
        .await
        .unwrap();
    assert!(stopped.attachment.is_none());
    assert_eq!(stopped.restart_pending, Some(false));

    let mut state = supervisor.snapshots();
    let launch = supervisor.clone();
    let start = tokio::spawn(async move { launch.ensure_ready().await });
    state.wait_for(|s| s.state == State::Booting).await.unwrap();
    let starting = supervisor
        .observe_network(NetworkMode::Userspace)
        .await
        .unwrap();
    assert!(starting.attachment.is_none());
    assert_eq!(starting.restart_pending, None);

    start.await.unwrap().unwrap();
    let fallback = supervisor.observe_network(NetworkMode::Auto).await.unwrap();
    let changed = supervisor
        .observe_network(NetworkMode::Userspace)
        .await
        .unwrap();
    assert_eq!(fallback.restart_pending, Some(false));
    assert_eq!(changed.restart_pending, Some(true));
    assert_eq!(fallback.attachment, changed.attachment);
    let observed = fallback.attachment.map(|(boot, _)| boot);
    let trace = trace.lock().unwrap().clone();
    assert_eq!(
        observed,
        Some(BootNetwork {
            active: NetworkSpec::Userspace,
            result: NetworkResult::Fallback(EgressStage::Route)
        })
    );
    assert_eq!(
        trace,
        [
            "native",
            "probe",
            "probe",
            "retire",
            "userspace",
            "probe",
            "engine"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn returns_to_native_without_starting_docker_on_an_unverified_candidate() {
    let (result, observed, trace) = boot(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        vec![
            Some(vec![EgressStage::Tcp; 2]),
            Some(vec![EgressStage::Tls]),
            None,
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(
        observed.unwrap().result,
        NetworkResult::AlternateFailed {
            native: EgressStage::Tcp,
            alternate: AlternateFailure::Egress(EgressStage::Tls),
        }
    );
    assert_eq!(
        trace,
        [
            "native",
            "probe",
            "probe",
            "retire",
            "userspace",
            "probe",
            "retire",
            "native",
            "engine"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn never_switches_a_healthy_pinned_offline_or_incompatible_guest() {
    for (host, mode, stages, want, trace) in [
        (
            Some(Arc::new(AvailableHost) as Arc<dyn HostCheck>),
            NetworkSpec::Nat,
            Some(vec![EgressStage::Healthy]),
            NetworkResult::NativeHealthy,
            vec!["native", "probe", "engine"],
        ),
        (
            Some(Arc::new(AvailableHost) as Arc<dyn HostCheck>),
            NetworkSpec::Nat,
            Some(vec![EgressStage::Tls]),
            NetworkResult::NativeDegraded(EgressStage::Tls),
            vec!["native", "probe", "engine"],
        ),
        (
            Some(Arc::new(OfflineHost) as Arc<dyn HostCheck>),
            NetworkSpec::Nat,
            Some(vec![]),
            NetworkResult::Unavailable,
            vec!["native", "engine"],
        ),
        (
            Some(Arc::new(AvailableHost) as Arc<dyn HostCheck>),
            NetworkSpec::Nat,
            None,
            NetworkResult::Unavailable,
            vec!["native", "engine"],
        ),
        (
            None,
            NetworkSpec::Nat,
            Some(vec![]),
            NetworkResult::Unchecked,
            vec!["native", "engine"],
        ),
        (
            None,
            NetworkSpec::Userspace,
            Some(vec![]),
            NetworkResult::Unchecked,
            vec!["userspace", "engine"],
        ),
    ] {
        let (result, observed, steps) =
            boot(host, mode, FakeScenario::default(), vec![stages]).await;
        result.unwrap();
        assert_eq!(
            observed,
            Some(BootNetwork {
                active: mode,
                result: want
            })
        );
        assert_eq!(steps, trace);
    }
}

#[tokio::test(start_paused = true)]
async fn failed_stop_never_reuses_the_writable_disk_for_a_candidate() {
    let (result, observed, trace) = boot(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default().with_failed_stops(2),
        vec![Some(vec![EgressStage::Dns; 2])],
    )
    .await;
    assert!(
        matches!(result, Err(SupervisorError::Boot(failure)) if failure.stage == BootStage::Network)
    );
    assert_eq!(observed, None);
    assert_eq!(trace, ["native", "probe", "probe", "retire"]);
}

#[tokio::test(start_paused = true)]
async fn a_candidate_with_unpublished_start_must_stop_before_native_disk_reuse() {
    let queued = FakeScenario::default()
        .with_start_publication_delay(STEP * 50)
        .with_failed_stops(1);
    let (result, observed, trace) = boot_with_candidate(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        Some(queued),
        vec![Some(vec![EgressStage::Route; 2]), None],
    )
    .await;
    assert!(
        matches!(result, Err(SupervisorError::Boot(failure)) if failure.stage == BootStage::Network)
    );
    assert_eq!(observed, None);
    assert_eq!(trace, ["native", "probe", "probe", "retire", "userspace"]);
}

#[tokio::test(start_paused = true)]
async fn helper_exit_during_engine_start_keeps_docker_but_clears_recovery() {
    let failing = FakeScenario::default().with_network_unavailable_after(STEP);
    let (result, observed, trace) = boot_with_candidate(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        Some(failing),
        vec![
            Some(vec![EgressStage::Route; 2]),
            Some(vec![EgressStage::Healthy]),
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(
        observed,
        Some(BootNetwork {
            active: NetworkSpec::Userspace,
            result: NetworkResult::AlternateFailed {
                native: EgressStage::Route,
                alternate: AlternateFailure::Transport,
            },
        })
    );
    assert_eq!(
        trace,
        [
            "native",
            "probe",
            "probe",
            "retire",
            "userspace",
            "probe",
            "engine"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_stalled_alternate_reserves_time_to_restore_local_docker() {
    let stalled = FakeScenario::default().with_start_publication_delay(STEP * 50);
    let started = tokio::time::Instant::now();
    let (result, observed, trace) = boot_with_candidate(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        Some(stalled),
        vec![Some(vec![EgressStage::Route; 2]), None],
    )
    .await;
    result.unwrap();
    assert!(started.elapsed() <= config().boot_timeout);
    assert_eq!(
        observed,
        Some(BootNetwork {
            active: NetworkSpec::Nat,
            result: NetworkResult::AlternateFailed {
                native: EgressStage::Route,
                alternate: AlternateFailure::Boot(BootStage::Start),
            },
        })
    );
    assert_eq!(
        trace,
        [
            "native",
            "probe",
            "probe",
            "retire",
            "userspace",
            "native",
            "engine"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn the_alternate_can_verify_after_a_single_transient_tcp_failure() {
    let (result, observed, trace) = boot(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        vec![
            Some(vec![EgressStage::Route; 2]),
            Some(vec![EgressStage::Tcp, EgressStage::Healthy]),
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(
        observed,
        Some(BootNetwork {
            active: NetworkSpec::Userspace,
            result: NetworkResult::Fallback(EgressStage::Route),
        })
    );
    assert_eq!(
        trace,
        [
            "native",
            "probe",
            "probe",
            "retire",
            "userspace",
            "probe",
            "probe",
            "engine"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn live_failures_are_confirmed_and_running_or_unknown_workloads_stay_online() {
    use EgressStage::{Healthy, Tcp};
    for count in [Some(2), None] {
        let (supervisor, trace) = fixture_workloads(
            Some(Arc::new(AvailableHost)),
            NetworkSpec::Nat,
            FakeScenario::default(),
            None,
            vec![Some(vec![Healthy, Tcp, Tcp, Healthy])],
            count,
        );
        supervisor.ensure_ready().await.unwrap();
        let mut changes = supervisor.events();
        let mut check = supervisor.live_network_check().await.unwrap().unwrap();
        let result = check.run().await;
        assert_eq!(result, NetworkResult::RestartForUserspace(Tcp));
        supervisor
            .apply_network_check(check, result, true)
            .await
            .unwrap();
        let change = changes.recv().await.unwrap();
        assert_eq!(change.current, change.previous);
        assert_eq!(change.network.unwrap().result, result);
        let status = supervisor.observe_network(NetworkMode::Auto).await.unwrap();
        assert_eq!(status.restart_pending, Some(true));
        assert_eq!(status.attachment.unwrap().0.active, NetworkSpec::Nat);
        let mut check = supervisor.live_network_check().await.unwrap().unwrap();
        let recovered = check.run().await;
        supervisor
            .apply_network_check(check, recovered, true)
            .await
            .unwrap();
        assert_eq!(
            supervisor
                .observe_network(NetworkMode::Auto)
                .await
                .unwrap()
                .restart_pending,
            Some(false)
        );
        assert!(!trace.lock().unwrap().contains(&"retire"));
    }
}

#[tokio::test(start_paused = true)]
async fn empty_vm_requalifies_userspace_before_releasing_engine() {
    use EgressStage::{Healthy, Tcp};
    let (supervisor, trace) = fixture_workloads(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        None,
        vec![
            Some(vec![Healthy, Tcp, Tcp]),
            Some(vec![Tcp, Tcp]),
            Some(vec![Healthy]),
        ],
        Some(0),
    );
    supervisor.ensure_ready().await.unwrap();
    let mut check = supervisor.live_network_check().await.unwrap().unwrap();
    let result = check.run().await;
    supervisor
        .apply_network_check(check, result, true)
        .await
        .unwrap();
    supervisor.ensure_ready().await.unwrap();
    let network = supervisor.snapshot().network.unwrap();
    assert_eq!(network.active, NetworkSpec::Userspace);
    assert_eq!(network.result, NetworkResult::Fallback(Tcp));
    assert_eq!(
        trace
            .lock()
            .unwrap()
            .iter()
            .filter(|step| **step == "engine")
            .count(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn helper_exit_after_readiness_publishes_loss_and_keeps_docker_ready() {
    let (supervisor, _) = fixture(
        None,
        NetworkSpec::Userspace,
        FakeScenario::default().with_network_unavailable_after(STEP * 8),
        None,
        vec![None],
    );
    supervisor.ensure_ready().await.unwrap();
    let mut events = supervisor.events();
    loop {
        let event = events.recv().await.unwrap();
        if event
            .network
            .is_some_and(|n| n.result == NetworkResult::TransportUnavailable)
        {
            assert_eq!(event.previous, event.current);
            break;
        }
    }
    assert_eq!(supervisor.snapshot().state, State::Ready);
}

#[tokio::test(start_paused = true)]
async fn transient_failures_and_policy_answers_never_schedule_a_live_switch() {
    use EgressStage::{Dns, Healthy, Policy, Tcp, Tls};
    for (samples, expected) in [
        (vec![Tcp, Healthy], NetworkResult::NativeHealthy),
        (vec![Tcp, Dns], NetworkResult::NativeDegraded(Dns)),
        (vec![Policy], NetworkResult::NativeDegraded(Policy)),
        (vec![Tls], NetworkResult::NativeDegraded(Tls)),
    ] {
        let stages = std::iter::once(Healthy).chain(samples).collect();
        let (supervisor, trace) = fixture_workloads(
            Some(Arc::new(AvailableHost)),
            NetworkSpec::Nat,
            FakeScenario::default(),
            None,
            vec![Some(stages)],
            Some(0),
        );
        supervisor.ensure_ready().await.unwrap();
        let mut check = supervisor.live_network_check().await.unwrap().unwrap();
        let result = check.run().await;
        assert_eq!(result, expected);
        supervisor
            .apply_network_check(check, result, true)
            .await
            .unwrap();
        assert_eq!(
            supervisor
                .observe_network(NetworkMode::Auto)
                .await
                .unwrap()
                .restart_pending,
            Some(false)
        );
        assert!(!trace.lock().unwrap().contains(&"retire"));
    }
}

#[tokio::test(start_paused = true)]
async fn pending_settings_and_superseded_session_cannot_authorize_a_restart() {
    use EgressStage::{Healthy, Tcp};
    let (supervisor, trace) = fixture_workloads(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
        None,
        vec![Some(vec![Healthy, Tcp, Tcp]), Some(vec![Healthy])],
        Some(0),
    );
    supervisor.ensure_ready().await.unwrap();
    let stale = supervisor.live_network_check().await.unwrap().unwrap();
    let mut check = supervisor.live_network_check().await.unwrap().unwrap();
    let result = check.run().await;
    supervisor
        .apply_network_check(check, result, false)
        .await
        .unwrap();
    assert!(
        supervisor
            .snapshot()
            .network
            .unwrap()
            .result
            .restart_pending(NetworkMode::Auto)
    );
    assert!(!trace.lock().unwrap().contains(&"retire"));
    supervisor.restart().await.unwrap();
    supervisor
        .apply_network_check(stale, result, true)
        .await
        .unwrap();
    assert_eq!(
        supervisor.snapshot().network.unwrap().result,
        NetworkResult::NativeHealthy
    );
    assert_eq!(
        trace
            .lock()
            .unwrap()
            .iter()
            .filter(|step| **step == "retire")
            .count(),
        1
    );
}

struct SlowHost;

#[async_trait]
impl HostCheck for SlowHost {
    async fn verified_target(&self) -> Option<Target> {
        tokio::time::sleep(Duration::from_secs(3)).await;
        Some(Target::fixture(TARGET))
    }
}

#[tokio::test(start_paused = true)]
async fn pinned_health_is_observed_after_readiness_and_on_handoffs_without_restart() {
    for (pin, active, unhealthy) in [
        (
            NetworkMode::Nat,
            NetworkSpec::Nat,
            NetworkResult::NativeDegraded(EgressStage::Tcp),
        ),
        (
            NetworkMode::Userspace,
            NetworkSpec::Userspace,
            NetworkResult::UserspaceDegraded(EgressStage::Tcp),
        ),
    ] {
        let (supervisor, trace) = fixture_mode(
            Some(Arc::new(SlowHost)),
            active,
            FakeScenario::default(),
            None,
            vec![Some(vec![EgressStage::Tcp, EgressStage::Healthy])],
            Some(0),
            pin,
        );
        let before = tokio::time::Instant::now();
        let mut events = supervisor.events();
        supervisor.ensure_ready().await.unwrap();
        assert!(before.elapsed() < Duration::from_secs(3));
        loop {
            let event = events.recv().await.unwrap();
            if event
                .network
                .is_some_and(|network| network.result == unhealthy)
            {
                break;
            }
        }
        let mut check = supervisor.live_network_check().await.unwrap().unwrap();
        let result = check.run().await;
        assert_eq!(result, NetworkResult::PinnedHealthy);
        supervisor
            .apply_network_check(check, result, true)
            .await
            .unwrap();
        assert_eq!(
            supervisor
                .observe_network(pin)
                .await
                .unwrap()
                .restart_pending,
            Some(false)
        );
        assert_eq!(supervisor.snapshot().network.unwrap().active, active);
        assert_eq!(
            supervisor.snapshot().network.unwrap().result,
            NetworkResult::PinnedHealthy
        );
        assert!(!trace.lock().unwrap().contains(&"retire"));
    }
}

#[tokio::test(start_paused = true)]
async fn a_pin_with_incomparable_policy_reports_unavailable_without_a_guest_probe() {
    for host in [None, Some(Arc::new(OfflineHost) as Arc<dyn HostCheck>)] {
        let (supervisor, trace) = fixture_mode(
            host,
            NetworkSpec::Userspace,
            FakeScenario::default(),
            None,
            vec![Some(vec![])],
            Some(0),
            NetworkMode::Userspace,
        );
        let mut events = supervisor.events();
        supervisor.ensure_ready().await.unwrap();
        loop {
            let event = events.recv().await.unwrap();
            if event
                .network
                .is_some_and(|network| network.result == NetworkResult::Unavailable)
            {
                break;
            }
        }
        assert_eq!(trace.lock().unwrap().as_slice(), ["userspace", "engine"]);
    }
}
