//! Limit trusted maintenance work to one live permit owned by the supervisor.

use tokio::sync::oneshot;

use super::{Supervisor, SupervisorError, actor::Command};

/// Exclusive lifecycle admission held until this value is dropped.
///
/// Keep the permit with the worker through the entire maintenance operation. A
/// successful stop releases the VM and its storage handle. Existing Docker
/// streams are attached only to that VM. The permit cannot be cloned.
#[must_use = "dropping the permit releases maintenance admission"]
pub struct LifecyclePermit {
    pub(super) owner: Supervisor,
    pub(super) id: u64,
    pub(super) _release: oneshot::Sender<()>,
}

impl LifecyclePermit {
    /// Stop the current VM and wait for cleanup, including a boot in progress.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        self.owner
            .request(|reply| Command::MaintenanceStop(self.id, reply))
            .await
    }

    /// Start one activation from Stopped without automatic supervisor retries.
    ///
    /// Dropping the permit before readiness cancels and cleans up the launch.
    pub async fn start(&self) -> Result<(), SupervisorError> {
        self.owner
            .request(|reply| Command::MaintenanceStart(self.id, reply))
            .await
    }
}
