//! Exercise the activation worker, durable state and shared supervisor with a fake VM.

use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::guest::v1::ConfigureNetwork;
use marsa_vm::{DiskSpec, Hypervisor, NetworkSpec, Vm, VmSpec, async_trait, fake::FakeHypervisor};
use tokio::{sync::Notify, time::Instant};
use tonic::Code;

use super::*;
use crate::supervisor::{GuestProbe, GuestSession, SupervisorConfig};

const REJECTED: &str = "http://candidate-rejected.invalid:8080";
const WORKING: &str = "http://approved.invalid:8080";

#[derive(Default)]
struct Probe {
    attempts: Mutex<Vec<Option<String>>>,
    gate: Mutex<Option<Arc<StartGate>>>,
}

#[derive(Default)]
struct StartGate {
    reached: Notify,
    proceed: Notify,
}

#[async_trait]
impl GuestProbe for Probe {
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        self.handshake_for_boot(vm, None).await
    }

    async fn handshake_for_boot(
        &self,
        _: &dyn Vm,
        policy: Option<&ConfigureNetwork>,
    ) -> io::Result<Box<dyn GuestSession>> {
        let url = policy.and_then(|policy| policy.proxy.as_ref()?.https_url.clone());
        self.attempts.lock().unwrap().push(url.clone());
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.reached.notify_one();
            gate.proceed.notified().await;
        }
        if url.as_deref() == Some(REJECTED) {
            Err(io::ErrorKind::InvalidData.into())
        } else {
            Ok(Box::new(Session))
        }
    }
}

struct Session;

#[async_trait]
impl GuestSession for Session {
    async fn engine_ready(&mut self, _: &dyn Vm) -> io::Result<()> {
        Ok(())
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    api: TypedApi,
    probe: Arc<Probe>,
}

impl Fixture {
    async fn new() -> Self {
        use std::{fs, os::unix::fs::PermissionsExt};
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let paths = MarsaPaths::from_root(home.path().to_owned()).unwrap();
        for directory in [paths.state_dir(), paths.vm_dir()] {
            fs::create_dir(&directory).unwrap();
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut initial = Settings::default();
        initial.network.mode = NetworkMode::Userspace;
        let api = TypedApi::new(paths.clone(), nix::unistd::geteuid().as_raw(), initial).unwrap();
        let backend = Arc::new(FakeHypervisor::new(Default::default()));
        let probe = Arc::new(Probe::default());
        let api = api
            .with_packaged_probe(
                backend.capabilities(),
                probe.clone(),
                SupervisorConfig {
                    boot_timeout: Duration::from_secs(2),
                    stop_timeout: Duration::from_secs(2),
                    idle_timeout: Duration::from_secs(60),
                    idle_stop: false,
                    max_retries: 0,
                    initial_backoff: Duration::ZERO,
                    maximum_backoff: Duration::ZERO,
                },
                move |effective| Ok((backend.clone(), spec(&paths, effective))),
            )
            .await
            .unwrap();
        Self {
            _home: home,
            api,
            probe,
        }
    }

    fn supervisor(&self) -> &Supervisor {
        self.api.supervisor.as_ref().unwrap()
    }

    async fn draft(&self, proxy: &str) -> u64 {
        let proxy = proxy.to_owned();
        let (snapshot, ()) = self
            .api
            .edit_settings(None, move |settings, _| {
                settings.network.proxy.https = Some(proxy);
                Ok(())
            })
            .await
            .unwrap();
        snapshot.revision
    }

    async fn approval(&self, revision: u64) -> ActivateProxyRequest {
        let preview = self.api.preview_proxy(revision).await.unwrap();
        assert_eq!(preview.settings_revision, revision);
        ActivateProxyRequest {
            request_id: "22000000-0000-4000-8000-000000000001".into(),
            expected_settings_revision: revision,
            approval_token: preview.approval_token,
            approved: true,
        }
    }

    fn approved(&self) -> Option<String> {
        BootProxy::read(&self.api.inner.paths, self.api.inner.uid)
            .unwrap()
            .effective(Settings::default())
            .network
            .proxy
            .https
    }
}

fn spec(paths: &MarsaPaths, settings: &Settings) -> VmSpec {
    let readonly = |path: &str| DiskSpec {
        path: PathBuf::from(path),
        read_only: true,
    };
    VmSpec {
        kernel: "/fixture/Image".into(),
        initramfs: None,
        cmdline: "console=hvc0".into(),
        cpus: settings.vm.cpus.into(),
        memory_bytes: 512 * 1024 * 1024,
        root_disk: readonly("/fixture/rootfs.img"),
        data_disk: DiskSpec {
            path: paths.data_disk(),
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Userspace,
        console_log: paths.console_log(),
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_candidate_restarts_prior_approved_policy_and_keeps_the_new_draft() {
    let fixture = Fixture::new().await;
    let initial = fixture.draft(WORKING).await;
    let first = fixture.approval(initial).await;
    assert_eq!(
        fixture.api.apply_proxy(first).await.unwrap().outcome(),
        Outcome::Applied,
    );
    let changed = fixture.draft(REJECTED).await;
    let request = fixture.approval(changed).await;
    let result = fixture.api.apply_proxy(request).await.unwrap();
    assert_eq!(result.outcome(), Outcome::PriorEngineRestored);
    assert_eq!(result.failure_stage(), Stage::Start);
    assert_eq!(fixture.supervisor().snapshot().state, State::Ready);
    assert_eq!(fixture.approved().as_deref(), Some(WORKING));
    assert_eq!(
        fixture
            .api
            .inner
            .events
            .settings()
            .0
            .network
            .proxy
            .https
            .as_deref(),
        Some(REJECTED)
    );
    assert!(
        fixture
            .api
            .inner
            .events
            .settings_snapshot()
            .restart_required
    );
    assert_eq!(
        *fixture.probe.attempts.lock().unwrap(),
        [
            Some(WORKING.into()),
            Some(REJECTED.into()),
            Some(WORKING.into()),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn revision_bound_approval_cannot_cross_an_edit_or_lifecycle_change() {
    let fixture = Fixture::new().await;
    let first_revision = fixture.draft(WORKING).await;
    let first = fixture.approval(first_revision).await;
    let next_revision = fixture.draft(REJECTED).await;
    assert_eq!(
        fixture.api.apply_proxy(first).await.unwrap_err().code(),
        Code::Aborted
    );
    assert!(fixture.probe.attempts.lock().unwrap().is_empty());

    let second = fixture.approval(next_revision).await;
    fixture.supervisor().ensure_ready().await.unwrap();
    assert_eq!(
        fixture.api.apply_proxy(second).await.unwrap_err().code(),
        Code::Aborted
    );
    assert_eq!(*fixture.probe.attempts.lock().unwrap(), [None]);
    assert_eq!(fixture.approved(), None);
}

#[tokio::test(start_paused = true)]
async fn expired_or_forged_approval_does_not_disrupt_an_engine() {
    let fixture = Fixture::new().await;
    fixture.supervisor().ensure_ready().await.unwrap();
    let revision = fixture.draft(WORKING).await;
    let request = fixture.approval(revision).await;
    let mut forged = request.clone();
    forged.approval_token[0] ^= 1;
    assert_eq!(
        fixture.api.apply_proxy(forged).await.unwrap_err().code(),
        Code::Aborted
    );
    let expiry = Instant::now() + Duration::from_secs(APPROVAL_SECONDS.into());
    tokio::time::sleep_until(expiry).await;
    assert_eq!(
        fixture.api.apply_proxy(request).await.unwrap_err().code(),
        Code::Aborted
    );
    assert_eq!(*fixture.probe.attempts.lock().unwrap(), [None]);
    assert_eq!(fixture.supervisor().snapshot().state, State::Ready);
}

#[tokio::test]
async fn disconnect_during_candidate_keeps_exclusive_admission_and_replays_commit() {
    let fixture = Fixture::new().await;
    let revision = fixture.draft(WORKING).await;
    let confirmation = fixture.approval(revision).await;
    let api = fixture.api.clone();
    let request = confirmation.clone();
    let gate = Arc::new(StartGate::default());
    *fixture.probe.gate.lock().unwrap() = Some(gate.clone());

    let caller = tokio::spawn(async move {
        let owner = api.clone();
        let request_id = request.request_id.clone();
        api.inner
            .proxy_requests
            .run(&request_id, request.clone(), move || async move {
                owner.apply_proxy(request).await
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .supervisor()
            .connect_docker()
            .await
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock,
    );
    assert!(matches!(
        fixture.supervisor().restart().await,
        Err(SupervisorError::Busy)
    ));
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    gate.proceed.notify_one();
    let request_id = confirmation.request_id.clone();
    let result = fixture
        .api
        .inner
        .proxy_requests
        .run(&request_id, confirmation, || async {
            panic!("retry must join the original proxy activation")
        })
        .await
        .unwrap();
    assert_eq!(result.outcome(), Outcome::Applied);
    assert_eq!(fixture.approved().as_deref(), Some(WORKING));
    assert_eq!(
        *fixture.probe.attempts.lock().unwrap(),
        [Some(WORKING.into())]
    );
}

#[tokio::test(start_paused = true)]
async fn factory_reset_clears_the_approved_proxy_from_storage_and_runtime() {
    use marsa_proto::host::v1::reset_response::Result as FactoryResult;

    let fixture = Fixture::new().await;
    let revision = fixture.draft(WORKING).await;
    assert_eq!(
        fixture
            .api
            .apply_proxy(fixture.approval(revision).await)
            .await
            .unwrap()
            .outcome(),
        Outcome::Applied
    );
    assert_eq!(fixture.approved().as_deref(), Some(WORKING));
    let id = "22000000-0000-4000-8000-000000000002";
    let prepared = fixture.api.prepare_factory_reset(id.into()).await.unwrap();
    let Some(FactoryResult::Challenge(confirmation)) = prepared.result else {
        panic!("expected a factory confirmation");
    };
    let result = fixture
        .api
        .confirm_factory_reset(
            "22000000-0000-4000-8000-000000000003",
            confirmation.confirmation_token,
        )
        .await
        .unwrap();
    assert!(matches!(result.result, Some(FactoryResult::Completed(_))));
    assert_eq!(fixture.approved(), None);
    assert!(
        fixture
            .api
            .proxy
            .as_ref()
            .unwrap()
            .approved()
            .guest_configuration()
            .is_none()
    );
    assert_eq!(fixture.supervisor().snapshot().state, State::Stopped);
}

#[tokio::test(start_paused = true)]
async fn an_approved_proxy_must_be_explicitly_disabled_before_network_mode_changes() {
    let fixture = Fixture::new().await;
    let revision = fixture.draft(WORKING).await;
    assert_eq!(
        fixture
            .api
            .apply_proxy(fixture.approval(revision).await)
            .await
            .unwrap()
            .outcome(),
        Outcome::Applied
    );
    let error = fixture
        .api
        .edit_settings(None, |settings, _| {
            settings.network.mode = NetworkMode::Nat;
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert_eq!(
        fixture.api.inner.events.configured_network_mode(),
        NetworkMode::Userspace
    );
    assert_eq!(fixture.approved().as_deref(), Some(WORKING));
    assert_eq!(fixture.supervisor().snapshot().state, State::Ready);
}
