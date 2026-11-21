//! Confine native VM objects to a private GCD serial queue.

mod configuration;
mod native;
mod network;
mod spec;
mod state;
mod vsock;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use marsa_vm::{
    Capabilities, Hypervisor, NetworkInfo, Vm, VmError, VmEventStream, VmSpec, VmState,
    VsockStream, async_trait,
};
use tokio::sync::watch;

use native::{Controller, Power, StopRequest};
use state::backend;

/// The Apple silicon Virtualization.framework backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct VzHypervisor;

#[async_trait]
impl Hypervisor for VzHypervisor {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            name: "vz",
            memory_balloon: true,
            pause_resume: true,
            dynamic_shares: false,
            userspace_network: false,
        }
    }

    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError> {
        let prepared = tokio::task::spawn_blocking(move || spec::prepare(spec))
            .await
            .map_err(|error| backend("prepare", error.to_string()))??;
        let discover_network = prepared.discover_network;
        // Arm queue teardown before the first job so a cancelled create also releases native state.
        let mut vm = VzVm {
            controller: Controller::new(),
            network_probe: None,
        };
        vm.controller
            .call("create", move |controller| controller.create(prepared))
            .await?;
        if discover_network {
            vm.network_probe = Some(tokio::spawn(network::discover(
                Arc::downgrade(&vm.controller),
                vm.controller.lifecycle.watch(),
            )));
        }
        Ok(Box::new(vm))
    }
}

struct VzVm {
    controller: Arc<Controller>,
    network_probe: Option<tokio::task::JoinHandle<()>>,
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
        self.change_power(Power::Pause).await
    }

    async fn resume(&mut self) -> Result<(), VmError> {
        self.change_power(Power::Resume).await
    }

    async fn stop(&mut self, timeout: Duration) -> Result<(), VmError> {
        let start = Instant::now();
        let mut state = self.controller.lifecycle.watch();
        loop {
            let remaining = || timeout.saturating_sub(start.elapsed());
            let expired = |_| VmError::Timeout {
                operation: "stop",
                limit: timeout,
            };
            match self
                .controller
                .call("request stop", |controller| controller.request_stop())
                .await?
            {
                StopRequest::WaitForStart => {
                    tokio::time::timeout(
                        remaining(),
                        state.wait_for(|state| *state != VmState::Starting),
                    )
                    .await
                    .map_err(expired)?
                    .map_err(|_| backend("stop", "the VZ state channel closed"))?;
                }
                StopRequest::WaitForPowerChange(mut finished) => {
                    tokio::time::timeout(remaining(), finished.wait_for(|finished| *finished))
                        .await
                        .map_err(expired)?
                        .map_err(|_| backend("stop", "the VZ power transition channel closed"))?;
                }
                StopRequest::Graceful => {
                    if let Ok(result) = tokio::time::timeout(remaining(), stopped(&mut state)).await
                    {
                        return result;
                    }
                    break;
                }
                StopRequest::Force => break,
            }
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

    async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmError> {
        vsock::connect(&self.controller, port).await
    }

    async fn set_memory_target(&self, bytes: u64) -> Result<(), VmError> {
        self.controller
            .call("set memory target", move |controller| {
                controller.set_memory_target(bytes)
            })
            .await
    }

    fn network_info(&self) -> Option<NetworkInfo> {
        self.controller.lifecycle.network()
    }
}

impl VzVm {
    async fn change_power(&self, power: Power) -> Result<(), VmError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.controller
            .submit(move |controller| controller.change_power(power, reply));
        result
            .await
            .map_err(|_| backend(power.name(), "the VZ queue lost the transition result"))?
    }
}

impl Drop for VzVm {
    fn drop(&mut self) {
        if let Some(probe) = &self.network_probe {
            probe.abort();
        }
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
