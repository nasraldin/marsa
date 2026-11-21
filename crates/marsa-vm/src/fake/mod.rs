//! Simulate lifecycle and guest connections on Tokio without an actual virtual machine.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use tokio::{
    io,
    task::{JoinHandle, JoinSet},
    time::sleep,
};

use crate::{
    Capabilities, Hypervisor, NetworkInfo, Vm, VmError, VmEventStream, VmSpec, VmState,
    VsockStream, async_trait,
};

mod lifecycle;
mod script;
use lifecycle::Lifecycle;
pub use script::AgentStep;

const CONNECTION_BUFFER: usize = 16 * 1024;

/// Deterministic behavior assigned to each VM made by a fake backend.
#[derive(Debug, Default, Clone)]
pub struct FakeScenario {
    start_timeout: Option<Duration>,
    exit_after: Option<Duration>,
    failed_stops: u32,
    agents: BTreeMap<u32, Vec<AgentStep>>,
    network_info: Option<NetworkInfo>,
}

impl FakeScenario {
    /// Have startup fail after a simulated deadline.
    pub fn with_start_timeout(mut self, limit: Duration) -> Self {
        self.start_timeout = Some(limit);
        self
    }

    /// Make the running guest exit unexpectedly after this delay.
    pub fn with_exit_after(mut self, delay: Duration) -> Self {
        self.exit_after = Some(delay);
        self
    }

    /// Fail this many stop requests before allowing shutdown.
    pub fn with_failed_stops(mut self, count: u32) -> Self {
        self.failed_stops = count;
        self
    }

    /// Replay the provided script on each connection to this guest port.
    pub fn with_agent(mut self, port: u32, steps: Vec<AgentStep>) -> Self {
        self.agents.insert(port, steps);
        self
    }

    /// Return these network facts from the simulated running guest.
    pub fn with_network_info(mut self, info: NetworkInfo) -> Self {
        self.network_info = Some(info);
        self
    }
}

/// Hypervisor for deterministic supervisor, API, and compatibility tests.
#[derive(Debug, Default, Clone)]
pub struct FakeHypervisor {
    scenario: Arc<FakeScenario>,
}

impl FakeHypervisor {
    /// Create an in-memory backend that runs on a Tokio runtime.
    pub fn new(scenario: FakeScenario) -> Self {
        Self {
            scenario: Arc::new(scenario),
        }
    }
}

#[async_trait]
impl Hypervisor for FakeHypervisor {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            name: "fake",
            memory_balloon: true,
            pause_resume: true,
            dynamic_shares: false,
            userspace_network: true,
        }
    }

    async fn create(&self, spec: VmSpec) -> Result<Box<dyn Vm>, VmError> {
        spec.validate()?;
        Ok(Box::new(FakeVm {
            scenario: self.scenario.clone(),
            lifecycle: Arc::new(Lifecycle::new()),
            connections: Arc::new(Mutex::new(JoinSet::new())),
            exit: None,
            memory_bytes: spec.memory_bytes,
            failed_stops: self.scenario.failed_stops,
        }))
    }
}

struct FakeVm {
    scenario: Arc<FakeScenario>,
    lifecycle: Arc<Lifecycle>,
    connections: Arc<Mutex<JoinSet<()>>>,
    exit: Option<JoinHandle<()>>,
    memory_bytes: u64,
    failed_stops: u32,
}

#[async_trait]
impl Vm for FakeVm {
    async fn start(&mut self) -> Result<(), VmError> {
        self.lifecycle.transition(
            "start",
            &[VmState::Created, VmState::Stopped],
            VmState::Starting,
        )?;
        if let Some(limit) = self.scenario.start_timeout {
            sleep(limit).await;
            self.lifecycle
                .transition("start", &[VmState::Starting], VmState::Failed)?;
            return Err(VmError::Timeout {
                operation: "start",
                limit,
            });
        }
        self.lifecycle
            .transition("start", &[VmState::Starting], VmState::Running)?;
        if let Some(delay) = self.scenario.exit_after {
            let lifecycle = self.lifecycle.clone();
            let connections = self.connections.clone();
            self.exit = Some(tokio::spawn(async move {
                sleep(delay).await;
                if lifecycle.exit_unexpectedly() {
                    lock(&connections).abort_all();
                }
            }));
        }
        Ok(())
    }

    async fn pause(&mut self) -> Result<(), VmError> {
        self.lifecycle
            .transition("pause", &[VmState::Running], VmState::Paused)
    }

    async fn resume(&mut self) -> Result<(), VmError> {
        self.lifecycle
            .transition("resume", &[VmState::Paused], VmState::Running)
    }

    async fn stop(&mut self, _timeout: Duration) -> Result<(), VmError> {
        if self.state() == VmState::Stopped {
            return Ok(());
        }
        if self.failed_stops > 0 {
            self.failed_stops -= 1;
            return Err(VmError::Backend {
                operation: "stop",
                detail: "scripted stop failure".into(),
            });
        }
        self.lifecycle.transition(
            "stop",
            &[
                VmState::Created,
                VmState::Starting,
                VmState::Running,
                VmState::Paused,
                VmState::Failed,
            ],
            VmState::Stopping,
        )?;
        self.abort_tasks();
        self.lifecycle
            .transition("stop", &[VmState::Stopping], VmState::Stopped)
    }

    fn state(&self) -> VmState {
        self.lifecycle.state()
    }

    fn events(&self) -> VmEventStream {
        self.lifecycle.events()
    }

    async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmError> {
        let mut connections = lock(&self.connections);
        self.lifecycle.running("connect to guest vsock")?;
        let steps = self
            .scenario
            .agents
            .get(&port)
            .ok_or(VmError::GuestPortUnavailable { port })?;
        while connections.try_join_next().is_some() {}
        let (host, guest) = io::duplex(CONNECTION_BUFFER);
        connections.spawn(script::serve(guest, steps.clone()));
        Ok(Box::new(host))
    }

    async fn set_memory_target(&self, bytes: u64) -> Result<(), VmError> {
        self.lifecycle.running("set guest memory target")?;
        if bytes == 0 || bytes > self.memory_bytes {
            return Err(VmError::InvalidSpec {
                field: "memory_bytes",
                reason: "target must be within configured guest memory".into(),
            });
        }
        Ok(())
    }

    fn network_info(&self) -> Option<NetworkInfo> {
        matches!(self.state(), VmState::Running | VmState::Paused)
            .then(|| self.scenario.network_info.clone())
            .flatten()
    }
}

impl FakeVm {
    fn abort_tasks(&mut self) {
        if let Some(exit) = self.exit.take() {
            exit.abort();
        }
        lock(&self.connections).abort_all();
    }
}

impl Drop for FakeVm {
    fn drop(&mut self) {
        self.abort_tasks();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
