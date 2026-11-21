//! Coordinate one VM boot, its readiness checks, and lifecycle requests.

use std::{io, sync::Arc, time::Duration};

use marsa_vm::{Hypervisor, Vm, VmError, VmSpec, async_trait};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

mod actor;
mod boot;
mod platform;

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
    /// Construct the VM and validate its devices.
    Create,
    /// Start the hypervisor.
    Start,
    /// Negotiate the versioned guest control channel.
    Handshake,
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
    /// Current engine state.
    pub state: State,
    /// Most recent failure from this activation, retained until a successful start.
    pub failure: Option<Arc<BootFailure>>,
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
}

/// Bounded startup, shutdown, retry, and idle behavior.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Deadline across VM start, handshake, and Docker health.
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
    #[error("guest failed during {:?}", .0.stage)]
    Boot(Arc<BootFailure>),
    /// A stop request canceled an outstanding readiness request.
    #[error("guest activation was stopped")]
    Stopped,
    /// The supervisor has shut down.
    #[error("guest supervisor is closed")]
    Closed,
    /// The readiness queue has reached its bound.
    #[error("too many requests are waiting for guest readiness")]
    Busy,
    /// An operation requires a different engine state.
    #[error("cannot {operation} the engine in state {state:?}")]
    InvalidState {
        /// Requested operation.
        operation: &'static str,
        /// Observed supervisor state.
        state: State,
    },
    /// The hypervisor rejected a lifecycle request.
    #[error(transparent)]
    Vm(#[from] Arc<VmError>),
}

/// Negotiate a control session over the backend's guest transport.
#[async_trait]
pub trait GuestProbe: Send + Sync {
    /// Establish the versioned guest session.
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>>;
}

/// Protocol session retained as long as the ready guest is running.
#[async_trait]
pub trait GuestSession: Send {
    /// Verify agent engine health and the Docker API ping.
    async fn engine_ready(&mut self, vm: &dyn Vm) -> io::Result<()>;
}

/// Shared client of one per-user VM supervisor.
#[derive(Clone)]
pub struct Supervisor {
    commands: mpsc::Sender<actor::Command>,
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
        let (commands, input) = mpsc::channel(actor::CHANNEL_CAPACITY);
        let (output, snapshot) = watch::channel(Snapshot {
            state: State::Stopped,
            failure: None,
        });
        let (events, _) = broadcast::channel(actor::CHANNEL_CAPACITY);
        tokio::spawn(
            actor::Actor::new(backend, probe, spec, config, input, output, events.clone()).run(),
        );
        Self {
            commands,
            snapshot,
            events,
        }
    }

    /// Wait for the shared activation attempt to make the engine ready.
    pub async fn ensure_ready(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::EnsureReady).await
    }

    /// Stop the guest and cancel any retry or pending activation.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Stop).await
    }

    /// Suspend the guest for a host lifecycle event.
    pub async fn pause(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Pause).await
    }

    /// Resume the guest and re-establish agent and engine readiness.
    pub async fn resume(&self) -> Result<(), SupervisorError> {
        self.request(actor::Command::Resume).await
    }

    /// Report an observed Docker container count for idle detection.
    pub async fn report_containers(&self, count: u32) -> Result<(), SupervisorError> {
        self.request(|reply| actor::Command::ContainerCount(count, reply))
            .await
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
