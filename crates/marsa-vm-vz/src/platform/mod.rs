//! Confine native VM objects to a private GCD serial queue.

mod configuration;
mod native;
mod spec;
mod state;
mod userspace;
mod vsock;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use marsa_vm::{
    Capabilities, Hypervisor, NetworkInfo, NetworkSpec, Vm, VmError, VmEventStream, VmSpec,
    VmState, VsockStream, async_trait,
};
use tokio::sync::watch;

use native::{Controller, StopRequest};
use state::{backend, unsupported};
use userspace::Transport;

/// The Apple silicon Virtualization.framework backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct VzHypervisor;

impl VzHypervisor {
    /// Enable the user-space attachment using an absolute path to the trusted
    /// helper bundled with Marsa. A native NAT spec still uses Apple's NAT.
    pub fn with_userspace(helper: PathBuf) -> impl Hypervisor {
        Configured { helper }
    }
}

struct Configured {
    helper: PathBuf,
}

#[async_trait]
impl Hypervisor for Configured {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            userspace_network: true,
            ..VzHypervisor.capabilities()
        }
    }

    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError> {
        create(spec, Some(self.helper.clone())).await
    }
}

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
        create(spec, None).await
    }
}

async fn create(spec: VmSpec, helper: Option<PathBuf>) -> Result<Box<dyn Vm>, VmError> {
    let (prepared, network) = tokio::task::spawn_blocking(move || {
        let mode = spec.network;
        let prepared = spec::prepare(spec)?;
        let network = if mode == NetworkSpec::Userspace {
            let helper = helper.ok_or_else(|| unsupported("VZ userspace networking"))?;
            Some(Transport::new(&helper)?)
        } else {
            None
        };
        Ok::<_, VmError>((prepared, network))
    })
    .await
    .map_err(|error| backend("prepare", error.to_string()))??;
    let (transport, socket) = network.unzip();
    // Arm queue teardown before the first job so a cancelled create releases native state.
    let vm = VzVm {
        controller: Controller::new(),
        transport,
    };
    vm.controller
        .call("create", move |controller| {
            controller.create(prepared, socket)
        })
        .await?;
    Ok(Box::new(vm))
}

struct VzVm {
    controller: Arc<Controller>,
    transport: Option<Transport>,
}

#[async_trait]
impl Vm for VzVm {
    async fn start(&mut self) -> Result<(), VmError> {
        if let Some(transport) = &mut self.transport {
            let state = self.controller.lifecycle.current();
            if !matches!(state, VmState::Created | VmState::Stopped) {
                return Err(VmError::InvalidState {
                    operation: "start",
                    state,
                });
            }
            transport.start(&self.controller).await?;
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.controller
            .submit(move |controller| controller.start(reply));
        let result = result
            .await
            .unwrap_or_else(|_| Err(backend("start", "the VZ queue lost the start result")));
        if result.is_err()
            && let Some(transport) = &mut self.transport
        {
            transport.stop().await;
        }
        result
    }

    async fn pause(&mut self) -> Result<(), VmError> {
        Err(unsupported("VZ pause"))
    }

    async fn resume(&mut self) -> Result<(), VmError> {
        Err(unsupported("VZ resume"))
    }

    async fn stop(&mut self, timeout: Duration) -> Result<(), VmError> {
        let outcome = self.stop_vm(timeout).await;
        if matches!(self.state(), VmState::Stopped | VmState::Failed)
            && let Some(transport) = &mut self.transport
        {
            transport.stop().await;
        }
        outcome
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

    async fn set_memory_target(&self, _bytes: u64) -> Result<(), VmError> {
        Err(unsupported("VZ memory balloon"))
    }

    fn network_info(&self) -> Option<NetworkInfo> {
        None
    }
}

impl VzVm {
    async fn stop_vm(&mut self, timeout: Duration) -> Result<(), VmError> {
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
