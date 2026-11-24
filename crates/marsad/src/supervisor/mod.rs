//! Coordinate one VM boot, its readiness checks, and lifecycle requests.

use std::{io, net::Ipv4Addr, path::PathBuf, sync::Arc, time::Duration};

use marsa_core::settings::NetworkMode;
use marsa_proto::guest::v1::EgressStage;
use marsa_vm::{Hypervisor, NetworkInfo, Vm, VmError, VmSpec, async_trait};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

mod actor;
mod maintenance;
pub use maintenance::LifecyclePermit;
mod boot;
mod docker;
mod host_egress;
pub(crate) mod launch;
pub(crate) mod network;
mod platform;

pub use docker::DockerStream;
pub use network::{AlternateFailure, BootNetwork, NetworkResult};

#[derive(Default)]
pub(crate) struct NetworkObservation {
    pub(crate) attachment: Option<(BootNetwork, Option<NetworkInfo>)>,
    pub(crate) restart_pending: Option<bool>,
}

#[cfg(test)]
mod tests;

/// User-visible state of the managed engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No VM is accepting guest traffic.
    Stopped,
    /// Creating and starting a VM.
    Starting,
    /// Waiting for the guest agent and Docker.
    Booting,
    /// The guest agent and Docker are available.
    Ready,
    /// An available VM has reached its idle deadline.
    Idle,
    /// Guest execution is paused.
    Paused,
    /// Shutdown is in progress.
    Stopping,
    /// An attempt failed, with details retained in the snapshot.
    Failed,
}

/// Phase in which guest activation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    /// Validate the packaged guest and prepare its persistent disk.
    Prepare,
    /// Construct the VM and validate its devices.
    Create,
    /// Start the hypervisor.
    Start,
    /// Negotiate the versioned guest control channel.
    Handshake,
    /// The guest data disk uses a newer supported format than this guest understands.
    DataDisk,
    /// Qualify the network before Docker starts.
    Network,
    /// Wait for Docker to serve requests.
    Engine,
    /// Handle an unexpected exit of a running guest.
    Running,
    /// Resume the guest and re-establish readiness after a pause.
    Resume,
    /// Stop an active or partially launched VM.
    Shutdown,
}

/// Bounded diagnostics for the last failed attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootFailure {
    /// Stage at failure.
    pub stage: BootStage,
    /// Whether the operation reached its deadline.
    pub timed_out: bool,
    /// Diagnostic from the failing stage.
    pub detail: String,
    /// Bounded, sanitized text from the guest serial transcript.
    pub console_tail: String,
}

/// Current supervisor state and the last boot failure, if present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Changes on a lifecycle transition, permit grant or permit release.
    /// A revision belongs to this supervisor instance.
    pub lifecycle_revision: u64,
    /// Current engine state.
    pub state: State,
    /// Most recent failure from this activation, retained until a successful start.
    pub failure: Option<Arc<BootFailure>>,
    /// Active attachment and boot-time connectivity evidence.
    pub network: Option<BootNetwork>,
}

/// An ordered lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// State before the event.
    pub previous: State,
    /// State after the event.
    pub current: State,
    /// Failure attached to this event.
    pub failure: Option<Arc<BootFailure>>,
    /// Active attachment and boot-time connectivity evidence.
    pub network: Option<BootNetwork>,
}

/// Bounded startup, shutdown, retry, and idle behavior.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Deadline across VM start, network selection, and Docker health.
    /// Safe cleanup may continue past this deadline.
    pub boot_timeout: Duration,
    /// Grace period passed to the backend before forced stop.
    pub stop_timeout: Duration,
    /// Time without running containers or recent requests before idling.
    pub idle_timeout: Duration,
    /// Stop the VM when its idle timer expires.
    pub idle_stop: bool,
    /// Maximum additional launches across one explicit activation.
    pub max_retries: u32,
    /// Delay before the first automatic retry.
    pub initial_backoff: Duration,
    /// Upper bound for automatic retry delays.
    pub maximum_backoff: Duration,
}

/// Failure returned by an operation on the supervisor.
#[derive(Debug, Clone, Error)]
pub enum SupervisorError {
    /// All startup attempts were exhausted.
    #[error("guest failed during {:?}: {}", .0.stage, .0.detail)]
    Boot(Arc<BootFailure>),
    /// A stop request canceled an outstanding readiness request.
    #[error("guest activation was stopped")]
    Stopped,
    /// The supervisor has shut down.
    #[error("guest supervisor is closed")]
    Closed,
    /// The readiness queue reached its bound or maintenance has exclusive admission.
    #[error("guest lifecycle admission is busy")]
    Busy,
    /// The lifecycle changed after maintenance was previewed.
    #[error("guest lifecycle revision changed from {expected} to {actual}")]
    StaleRevision {
        /// Revision supplied with the maintenance preview.
        expected: u64,
        /// Current revision when the request reached the actor.
        actual: u64,
    },
    /// An operation requires a different engine state.
    #[error("cannot {operation} the engine in state {state:?}")]
    InvalidState {
        /// Requested operation.
        operation: &'static str,
        /// Observed supervisor state.
        state: State,
    },
    /// The running guest could not acknowledge a clock update after waking.
    #[error("guest clock synchronization failed: {0}")]
    Clock(#[source] Arc<io::Error>),
    /// The hypervisor rejected a lifecycle request.
    #[error(transparent)]
    Vm(#[from] Arc<VmError>),
}

/// Negotiate a control session over the backend's guest transport.
#[async_trait]
pub trait GuestProbe: Send + Sync {
    /// Observe running, paused and restarting workloads, or opt out of automatic observation.
    async fn active_containers(&self, _vm: &dyn Vm) -> Option<io::Result<u32>> {
        None
    }

    /// Establish the versioned guest session.
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>>;

    /// Establish a cold-boot session, provisioning approved policy before releasing Docker.
    async fn handshake_for_boot(
        &self,
        vm: &dyn Vm,
        policy: Option<&marsa_proto::guest::v1::ConfigureNetwork>,
    ) -> io::Result<Box<dyn GuestSession>> {
        if policy.is_some() {
            return Err(io::ErrorKind::Unsupported.into());
        }
        self.handshake(vm).await
    }
}

/// Protocol session retained as long as the ready guest is running.
#[async_trait]
pub trait GuestSession: Send {
    /// Whether this guest supports checking egress before starting the engine.
    fn can_probe_egress(&self) -> bool {
        false
    }

    /// Verify the configured HTTPS registry on the same IPv4 endpoint as the host.
    async fn probe_egress(&mut self, _target: Ipv4Addr) -> io::Result<EgressStage> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Verify agent engine health and the Docker API ping.
    async fn engine_ready(&mut self, vm: &dyn Vm) -> io::Result<()>;

    /// Update the wall clock of a guest whose session remained active through sleep.
    async fn synchronize_clock(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Ask the guest to stop Docker and unmount its data disk before powering off.
    async fn shutdown(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Wait for loss of the established control stream.
    async fn disconnected(&mut self) -> io::Error {
        std::future::pending().await
    }
}

type Observer = Arc<dyn Fn(&Transition) + Send + Sync>;

/// Shared client of one per-user VM supervisor.
#[derive(Clone)]
pub struct Supervisor {
    commands: mpsc::Sender<actor::Command>,
    docker_clients: watch::Sender<usize>,
    snapshot: watch::Receiver<Snapshot>,
    events: broadcast::Sender<Transition>,
}

impl Supervisor {
    /// Start the supervisor actor on the current Tokio runtime.
    pub fn spawn(
        backend: Arc<dyn Hypervisor>,
        probe: Arc<dyn GuestProbe>,
        spec: VmSpec,
        config: SupervisorConfig,
    ) -> Self {
        Self::spawn_inner(
            backend,
            probe,
            spec,
            config,
            None,
            network::Selection::new(false),
        )
    }

    pub(crate) fn spawn_observed(
        backend: Arc<dyn Hypervisor>,
        probe: Arc<dyn GuestProbe>,
        spec: VmSpec,
        config: SupervisorConfig,
        observer: Observer,
        selection: network::Selection,
    ) -> Self {
        Self::spawn_inner(backend, probe, spec, config, Some(observer), selection)
    }

    fn spawn_inner(
        backend: Arc<dyn Hypervisor>,
        probe: Arc<dyn GuestProbe>,
        spec: VmSpec,
        config: SupervisorConfig,
        observer: Option<Observer>,
        selection: network::Selection,
    ) -> Self {
        let console_log = spec.console_log.clone();
        let pause_resume = backend.capabilities().pause_resume;
        let launch = launch::Launch {
            backend,
            spec,
            selection,
            network_policy: None,
            applied: None,
        };
        Self::spawn_runtime(
            launch::Runtime {
                source: launch::Source::Fixed(Box::new(launch)),
                console_log,
                pause_resume,
            },
            probe,
            config,
            observer,
        )
    }

    pub(crate) fn spawn_packaged(
        prepare: launch::Prepare,
        console_log: PathBuf,
        pause_resume: bool,
        probe: Arc<dyn GuestProbe>,
        config: SupervisorConfig,
        observer: Observer,
    ) -> Self {
        Self::spawn_runtime(
            launch::Runtime {
                source: launch::Source::Package(prepare),
                console_log,
                pause_resume,
            },
            probe,
            config,
            Some(observer),
        )
    }

    fn spawn_runtime(
        launch: launch::Runtime,
        probe: Arc<dyn GuestProbe>,
        config: SupervisorConfig,
        observer: Option<Observer>,
    ) -> Self {
        let (commands, input) = mpsc::channel(actor::CHANNEL_CAPACITY);
        let (docker_clients, clients) = watch::channel(0);
        let (output, snapshot) = watch::channel(Snapshot {
            lifecycle_revision: 1,
            state: State::Stopped,
            failure: None,
            network: None,
        });
        let (events, _) = broadcast::channel(actor::CHANNEL_CAPACITY);
        tokio::spawn(
            actor::Actor::new(
                launch,
                probe,
                config,
                input,
                clients,
                actor::TransitionOutput {
                    snapshot: output,
                    events: events.clone(),
                    observer,
                },
            )
            .run(),
        );
        Self {
            commands,
            docker_clients,
            snapshot,
            events,
        }
    }

    /// Reserve lifecycle admission and optionally check the revision of a preview.
    ///
    /// Ordinary lifecycle and new Docker connects return Busy while the permit
    /// is held. A caller that needs the VM offline must await the permit's stop.
    pub async fn reserve(&self, expected: Option<u64>) -> Result<LifecyclePermit, SupervisorError> {
        let (release, released) = oneshot::channel();
        let (reply, result) = oneshot::channel();
        self.commands
            .send(actor::Command::Reserve {
                expected,
                release: released,
                reply,
            })
            .await
            .map_err(|_| SupervisorError::Closed)?;
        let id = result.await.map_err(|_| SupervisorError::Closed)??;
        Ok(LifecyclePermit {
            owner: self.clone(),
            id,
            _release: release,
        })
    }

    /// Wait for the shared activation attempt to make the engine ready.
    pub async fn ensure_ready(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::EnsureReady).await
    }

    /// Start the engine if necessary and hold a Docker API stream open against idle stop.
    pub async fn connect_docker(&self) -> io::Result<DockerStream> {
        let activity = docker::Activity::acquire(&self.docker_clients);
        let stream = loop {
            self.ensure_ready().await.map_err(|error| {
                let kind = match &error {
                    SupervisorError::Boot(failure) if failure.timed_out => io::ErrorKind::TimedOut,
                    SupervisorError::Busy => io::ErrorKind::WouldBlock,
                    _ => io::ErrorKind::NotConnected,
                };
                io::Error::new(kind, error)
            })?;
            match self.connect_running_docker().await {
                // An explicit restart or host pause can intervene after readiness.
                // Join that transition without overriding a completed user Stop.
                Err(error)
                    if error.kind() == io::ErrorKind::NotConnected
                        && matches!(
                            self.snapshot().state,
                            State::Starting | State::Booting | State::Paused
                        ) =>
                {
                    continue;
                }
                result => break result?,
            }
        };
        Ok(DockerStream {
            stream,
            _activity: activity,
        })
    }

    pub(crate) async fn connect_running_docker(&self) -> io::Result<marsa_vm::VsockStream> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(actor::Command::ConnectDocker(reply))
            .await
            .map_err(|_| io::ErrorKind::NotConnected)?;
        result.await.map_err(|_| io::ErrorKind::NotConnected)?
    }

    /// Stop the guest and cancel any retry or pending activation.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Stop).await
    }

    /// Atomically stop the current VM and request one new activation.
    pub async fn restart(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Restart).await
    }

    /// Suspend the guest for a host lifecycle event.
    pub async fn pause(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Pause).await
    }

    /// Resume the guest and re-establish agent and engine readiness.
    pub async fn resume(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Resume).await
    }

    pub(crate) async fn host_will_sleep(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::HostWillSleep).await
    }

    pub(crate) async fn host_woke(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::HostWoke).await
    }

    /// Report an observed Docker container count for idle detection.
    pub async fn report_containers(&self, count: u32) -> Result<(), SupervisorError> {
        self.request(|reply| actor::Command::ContainerCount(count, reply))
            .await
    }

    /// Apply a new idle policy to the running engine or its next boot.
    pub async fn set_idle_policy(
        &self,
        stop: bool,
        timeout: Duration,
    ) -> Result<(), SupervisorError> {
        self.request(|reply| actor::Command::IdlePolicy(stop, timeout, reply))
            .await
    }

    /// Read the live attachment and compare the activation's mode with the requested policy.
    pub(crate) async fn observe_network(
        &self,
        desired: NetworkMode,
    ) -> Result<NetworkObservation, SupervisorError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(actor::Command::Network(desired, reply))
            .await
            .map_err(|_| SupervisorError::Closed)?;
        result.await.map_err(|_| SupervisorError::Closed)
    }

    /// Read the current state, even if lifecycle events were missed.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.borrow().clone()
    }

    /// Subscribe to state updates, starting with the current snapshot.
    pub fn snapshots(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.clone()
    }

    /// Subscribe to future lifecycle transitions.
    pub fn events(&self) -> broadcast::Receiver<Transition> {
        self.events.subscribe()
    }

    async fn request(
        &self,
        command: impl FnOnce(actor::Reply) -> actor::Command,
    ) -> Result<(), SupervisorError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| SupervisorError::Closed)?;
        result.await.map_err(|_| SupervisorError::Closed)?
    }
}
