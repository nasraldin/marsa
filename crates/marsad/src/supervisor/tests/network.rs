//! Verify boot selection without external networks or a virtualization entitlement.

use std::{collections::VecDeque, net::Ipv4Addr, sync::Mutex};

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
}

#[async_trait]
impl GuestProbe for Probe {
    async fn handshake(&self, _: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        Ok(Box::new(Session {
            trace: self.trace.clone(),
            stages: self.attempts.lock().unwrap().pop_front().unwrap(),
        }))
    }
}

struct Session {
    trace: Trace,
    stages: Option<Vec<EgressStage>>,
}

#[async_trait]
impl GuestSession for Session {
    fn can_probe_egress(&self) -> bool {
        self.stages.is_some()
    }

    async fn probe_egress(&mut self, ip: Ipv4Addr) -> io::Result<EgressStage> {
        assert_eq!(ip, TARGET);
        self.trace.lock().unwrap().push("probe");
        Ok(self.stages.as_mut().unwrap().remove(0))
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
    let trace = Trace::default();
    let backend = Arc::new(Backend {
        trace: trace.clone(),
        scenario,
        userspace,
    });
    let probe = Arc::new(Probe {
        trace: trace.clone(),
        attempts: Mutex::new(attempts.into()),
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
    let supervisor =
        Supervisor::spawn_inner(backend, probe, spec, config(), None, Selection { host });
    let result = supervisor.ensure_ready().await;
    let network = supervisor.snapshot().network;
    let steps = trace.lock().unwrap().clone();
    (result, network, steps)
}

#[tokio::test(start_paused = true)]
async fn qualifies_userspace_before_releasing_the_engine() {
    let (result, observed, trace) = boot(
        Some(Arc::new(AvailableHost)),
        NetworkSpec::Nat,
        FakeScenario::default(),
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
