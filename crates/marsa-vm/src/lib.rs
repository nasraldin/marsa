//! Backend-independent VM contracts shared by the host daemon and hypervisors.

use std::{io, net::IpAddr, pin::Pin, time::Duration};

/// Define async methods on dynamically dispatched hypervisor implementations.
pub use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::Stream;

mod spec;
pub use spec::{DiskSpec, NetworkSpec, ShareSpec, VmSpec};

#[cfg(any(feature = "fake", test))]
pub mod fake;
#[cfg(test)]
mod tests;

/// Static features exposed by a hypervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Stable backend identifier used by settings and the host API.
    pub name: &'static str,
    /// Adjust a running guest's target memory.
    pub memory_balloon: bool,
    /// Suspend and resume a live guest.
    pub pause_resume: bool,
    /// Change host shares while the guest runs.
    pub dynamic_shares: bool,
    /// Attach a user-space network stack.
    pub userspace_network: bool,
}

/// Network facts known for a running guest attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    /// Guest address on its virtual network.
    pub address: IpAddr,
    /// Address of the guest's default gateway, when present.
    pub gateway: Option<IpAddr>,
}

/// Backend lifecycle, independent of the daemon's agent and engine readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    /// The configuration was created and the guest has not started.
    Created,
    /// The backend is starting the guest.
    Starting,
    /// The hypervisor has started guest execution.
    Running,
    /// Guest execution has been suspended.
    Paused,
    /// Shutdown is in progress.
    Stopping,
    /// Guest execution has ended.
    Stopped,
    /// The backend failed to start or the guest exited unexpectedly.
    Failed,
}

/// Native cause of guest execution ending outside a backend stop operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmExitCause {
    /// The backend received a guest power-off notification.
    GuestPowerOff,
    /// The backend reported a failure.
    Failure,
}

/// A lifecycle notification from the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmEvent {
    /// A backend state transition.
    StateChanged {
        /// State before the transition.
        previous: VmState,
        /// State after the transition.
        current: VmState,
    },
    /// The backend stopped guest execution outside a requested shutdown.
    UnexpectedExit {
        /// Native cause of the exit.
        cause: VmExitCause,
        /// Diagnostic supplied by the backend.
        detail: String,
    },
    /// The network transport failed while guest control remains available.
    NetworkUnavailable {
        /// Diagnostic supplied by the backend.
        detail: String,
    },
    /// The consumer lagged. Subscribe again and take a fresh state sample.
    EventsDropped {
        /// Number of lifecycle events the consumer missed.
        count: u64,
    },
}

/// Typed failure shared across VM implementations.
#[derive(Debug, Error)]
pub enum VmError {
    /// A configuration field violates a backend-independent or platform limit.
    #[error("invalid VM specification ({field}): {reason}")]
    InvalidSpec {
        /// Field that caused the error.
        field: &'static str,
        /// Description of the violated invariant.
        reason: String,
    },
    /// An operation is unavailable in the current lifecycle state.
    #[error("cannot {operation} the VM in state {state:?}")]
    InvalidState {
        /// Requested lifecycle operation.
        operation: &'static str,
        /// Observed backend state.
        state: VmState,
    },
    /// The backend cannot perform the requested operation.
    #[error("VM backend does not support {capability}")]
    Unsupported {
        /// Missing feature.
        capability: &'static str,
    },
    /// A bounded operation exceeded its deadline.
    #[error("VM operation {operation} exceeded {limit:?}")]
    Timeout {
        /// Timed-out operation.
        operation: &'static str,
        /// Configured deadline.
        limit: Duration,
    },
    /// No listener accepted the requested guest port.
    #[error("guest vsock port {port} is unavailable")]
    GuestPortUnavailable {
        /// Requested guest port number.
        port: u32,
    },
    /// A backend or platform API reported a failure.
    #[error("VM backend failed to {operation}: {detail}")]
    Backend {
        /// Operation attempted.
        operation: &'static str,
        /// Backend diagnostic.
        detail: String,
    },
    /// An underlying host I/O operation failed.
    #[error("VM I/O failed during {operation}: {source}")]
    Io {
        /// Operation attempted.
        operation: &'static str,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
}

/// Portable, bidirectional guest connection implemented by each backend.
pub trait VsockTransport: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> VsockTransport for T {}

/// Owned transport returned by a virtual socket connection.
pub type VsockStream = Box<dyn VsockTransport>;

/// Independently owned stream of backend state and failure events.
pub type VmEventStream = Pin<Box<dyn Stream<Item = VmEvent> + Send + 'static>>;

/// Factory for unstarted guest VMs.
#[async_trait]
pub trait Hypervisor: Send + Sync {
    /// Return the features available from this backend.
    fn capabilities(&self) -> Capabilities;

    /// Build and validate the guest configuration.
    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError>;
}

/// One guest VM owned by its supervisor.
#[async_trait]
pub trait Vm: Send + Sync {
    /// Start guest execution.
    async fn start(&mut self) -> Result<(), VmError>;
    /// Suspend guest execution.
    async fn pause(&mut self) -> Result<(), VmError>;
    /// Resume a suspended guest.
    async fn resume(&mut self) -> Result<(), VmError>;
    /// Ask the guest to shut down and force it off after the deadline.
    async fn stop(&mut self, timeout: Duration) -> Result<(), VmError>;
    /// Return the most recently observed backend state.
    fn state(&self) -> VmState;
    /// Subscribe to future lifecycle events.
    fn events(&self) -> VmEventStream;
    /// Open a bidirectional connection to a guest vsock port.
    async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmError>;
    /// Adjust the guest's memory target when supported.
    async fn set_memory_target(&self, bytes: u64) -> Result<(), VmError>;
    /// Return the network facts known by the backend.
    fn network_info(&self) -> Option<NetworkInfo>;
}
