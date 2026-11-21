//! Confine native VM objects to a private GCD serial queue.

mod configuration;
mod native;
mod spec;
mod state;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use marsa_vm::{
    Capabilities, Hypervisor, NetworkInfo, Vm, VmError, VmEventStream, VmSpec, VmState,
    VsockStream, async_trait,
};
use tokio::sync::watch;

use native::{Controller, StopRequest};
use state::{backend, unsupported};

/// The Apple silicon Virtualization.framework backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct VzHypervisor;

#[async_trait]
impl Hypervisor for VzHypervisor {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            name: "vz",
            memory_balloon: false,
            pause_resume: false,
            dynamic_shares: false,
            userspace_network: false,
        }
    }

    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError> {
        let prepared = tokio::task::spawn_blocking(move || spec::prepare(spec))
            .await
            .map_err(|error| backend("prepare", error.to_string()))??;
        // Arm queue teardown before the first job so a cancelled create also releases native state.
        let vm = VzVm {
            controller: Controller::new(),
        };
        vm.controller
            .call("create", move |controller| controller.create(prepared))
            .await?;
        Ok(Box::new(vm))
    }
}

struct VzVm {
    controller: Arc<Controller>,
}

#[async_trait]
impl Vm for VzVm {
    async fn start(&mut self) -> Result<(), VmError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.controller
            .submit(move |controller| controller.start(reply));
        result
            .await
            .map_err(|_| backend("start", "the VZ queue lost the start result"))?
    }

    async fn pause(&mut self) -> Result<(), VmError> {
        Err(unsupported("VZ pause"))
    }

    async fn resume(&mut self) -> Result<(), VmError> {
        Err(unsupported("VZ resume"))
    }

    async fn stop(&mut self, timeout: Duration) -> Result<(), VmError> {
        let start = Instant::now();
        let mut state = self.controller.lifecycle.watch();
        let request = |controller: &Arc<Controller>| controller.request_stop();
        let mut stop = self.controller.call("request stop", request).await?;
        if stop == StopRequest::WaitForStart {
            // A cancelled start future cannot cancel the native start. Wait for its callback.
            tokio::time::timeout(
                timeout.saturating_sub(start.elapsed()),
                state.wait_for(|state| *state != VmState::Starting),
            )
            .await
            .map_err(|_| VmError::Timeout {
                operation: "stop",
                limit: timeout,
            })?
            .map_err(|_| backend("stop", "the VZ state channel closed"))?;
            stop = self.controller.call("request stop", request).await?;
        }
        if stop == StopRequest::Graceful
            && let Ok(result) =
                tokio::time::timeout(timeout.saturating_sub(start.elapsed()), stopped(&mut state))
                    .await
        {
            return result;
        }
        self.controller
            .call("stop", |controller| controller.force_stop())
            .await?;
        stopped(&mut state).await
    }

    fn state(&self) -> VmState {
        self.controller.lifecycle.current()
    }

    fn events(&self) -> VmEventStream {
        self.controller.lifecycle.events()
    }

    async fn connect_vsock(&self, _port: u32) -> Result<VsockStream, VmError> {
        Err(unsupported("VZ vsock connections"))
    }

    async fn set_memory_target(&self, _bytes: u64) -> Result<(), VmError> {
        Err(unsupported("VZ memory balloon"))
    }

    fn network_info(&self) -> Option<NetworkInfo> {
        None
    }
}

impl Drop for VzVm {
    fn drop(&mut self) {
        self.controller.submit(|controller| controller.close());
    }
}

async fn stopped(state: &mut watch::Receiver<VmState>) -> Result<(), VmError> {
    let state = state
        .wait_for(|state| matches!(state, VmState::Stopped | VmState::Failed))
        .await
        .map_err(|_| backend("stop", "the VZ state channel closed"))?;
    if *state == VmState::Failed {
        return Err(backend(
            "stop",
            "Virtualization.framework failed during shutdown",
        ));
    }
    Ok(())
}
