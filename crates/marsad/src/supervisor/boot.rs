//! Keep ownership of a partial VM while timing out or canceling individual boot phases.

use std::{future::Future, io, sync::Arc, time::Duration};

use marsa_vm::{Hypervisor, Vm, VmError, VmEvent, VmEventStream, VmExitCause, VmSpec, VmState};
use tokio::{
    sync::{oneshot, watch},
    time::{Instant, timeout_at},
};
use tokio_stream::StreamExt;

use super::{BootNetwork, BootStage, GuestProbe, GuestSession, NetworkResult, network::Selection};

mod selection;

pub(super) struct LiveGuest {
    pub vm: Box<dyn Vm>,
    pub session: Option<Box<dyn GuestSession>>,
    pub events: VmEventStream,
    pub network: BootNetwork,
}

pub(super) enum Outcome {
    Ready(LiveGuest),
    Failed {
        vm: Option<Box<dyn Vm>>,
        issue: Issue,
    },
}

struct Failed {
    vm: Option<Box<dyn Vm>>,
    issue: Issue,
}

impl From<Failed> for Outcome {
    fn from(Failed { vm, issue }: Failed) -> Self {
        Self::Failed { vm, issue }
    }
}

pub(super) struct Issue {
    pub stage: BootStage,
    pub reason: Reason,
}

pub(super) enum Reason {
    Timeout,
    Failed(String),
    Canceled,
}

impl Issue {
    pub(super) fn failed(stage: BootStage, detail: impl Into<String>) -> Self {
        Self {
            stage,
            reason: Reason::Failed(detail.into()),
        }
    }
}

pub(super) struct Activation {
    pub(super) spec: VmSpec,
    pub(super) limit: Duration,
    pub(super) stop_limit: Duration,
    pub(super) selection: Selection,
}

pub(super) async fn run(
    backend: Arc<dyn Hypervisor>,
    probe: Arc<dyn GuestProbe>,
    activation: Activation,
    mut cancel: oneshot::Receiver<()>,
    progress: watch::Sender<BootStage>,
) -> Outcome {
    let result = selection::select_and_start(
        backend.as_ref(),
        probe.as_ref(),
        activation,
        &mut cancel,
        &progress,
    )
    .await;
    match result {
        Ok(guest) => Outcome::Ready(guest),
        Err(failure) => failure.into(),
    }
}

async fn attach(
    backend: &dyn Hypervisor,
    probe: &dyn GuestProbe,
    spec: VmSpec,
    deadline: Instant,
    cancel: &mut oneshot::Receiver<()>,
    progress: &watch::Sender<BootStage>,
) -> Result<LiveGuest, Failed> {
    let active = spec.network;
    progress.send_replace(BootStage::Create);
    let mut vm = phase(BootStage::Create, deadline, cancel, backend.create(spec))
        .await
        .map_err(|issue| Failed { vm: None, issue })?;
    let mut events = vm.events();
    if let Err(issue) = phase(BootStage::Start, deadline, cancel, vm.start()).await {
        return Err(Failed {
            vm: Some(vm),
            issue,
        });
    }
    progress.send_replace(BootStage::Handshake);
    let session = phase(
        BootStage::Handshake,
        deadline,
        cancel,
        during_guest_execution(vm.as_ref(), &mut events, probe.handshake(vm.as_ref())),
    )
    .await;
    match session {
        Ok(session) => Ok(LiveGuest {
            vm,
            session: Some(session),
            events,
            network: BootNetwork {
                active,
                result: NetworkResult::Unchecked,
            },
        }),
        Err(issue) => Err(Failed {
            vm: Some(vm),
            issue,
        }),
    }
}

async fn start_engine(
    mut guest: LiveGuest,
    deadline: Instant,
    cancel: &mut oneshot::Receiver<()>,
    progress: &watch::Sender<BootStage>,
) -> Result<LiveGuest, Failed> {
    let Some(session) = guest.session.as_deref_mut() else {
        return Err(Failed {
            vm: Some(guest.vm),
            issue: Issue::failed(BootStage::Engine, "guest session unavailable"),
        });
    };
    progress.send_replace(BootStage::Engine);
    let ready = phase(
        BootStage::Engine,
        deadline,
        cancel,
        during_guest_execution(
            guest.vm.as_ref(),
            &mut guest.events,
            session.engine_ready(guest.vm.as_ref()),
        ),
    )
    .await;
    match ready {
        Ok(()) => Ok(guest),
        Err(issue) => Err(Failed {
            vm: Some(guest.vm),
            issue,
        }),
    }
}

async fn phase<T, E: std::fmt::Display>(
    stage: BootStage,
    deadline: Instant,
    cancel: &mut oneshot::Receiver<()>,
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, Issue> {
    tokio::select! {
        biased;
        _ = cancel => Err(Issue { stage, reason: Reason::Canceled }),
        result = timeout_at(deadline, work) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(Issue::failed(stage, error.to_string())),
            Err(_) => Err(Issue { stage, reason: Reason::Timeout }),
        }
    }
}

async fn during_guest_execution<T>(
    vm: &dyn Vm,
    events: &mut VmEventStream,
    work: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::select! {
        result = work => result,
        detail = guest_exit(vm, events) => Err(io::Error::other(detail)),
    }
}

pub(super) async fn graceful_stop(
    mut guest: LiveGuest,
    probe: &dyn GuestProbe,
    limit: Duration,
) -> (Box<dyn Vm>, Duration, Option<VmError>) {
    let deadline = Instant::now() + limit;
    let outcome = timeout_at(deadline, async {
        if guest.vm.state() == VmState::Paused {
            guest.vm.resume().await.map_err(io::Error::other)?;
            guest.session = Some(probe.handshake(guest.vm.as_ref()).await?);
        }
        let Some(session) = guest.session.as_mut() else {
            return Ok(None);
        };
        session.shutdown().await?;
        Ok::<_, io::Error>(
            wait_for_power_off(guest.vm.as_ref(), &mut guest.events)
                .await
                .err(),
        )
    })
    .await;
    // Release the control stream before falling back to native stop.
    drop(guest.session);
    let remaining = deadline.saturating_duration_since(Instant::now());
    (
        guest.vm,
        remaining,
        outcome.ok().and_then(Result::ok).flatten(),
    )
}

async fn wait_for_power_off(vm: &dyn Vm, events: &mut VmEventStream) -> Result<(), VmError> {
    fn failure(detail: String) -> VmError {
        VmError::Backend {
            operation: "shut down",
            detail,
        }
    }
    loop {
        if vm.state() == VmState::Stopped {
            return Ok(());
        }
        match events.next().await {
            Some(VmEvent::UnexpectedExit {
                cause: VmExitCause::GuestPowerOff,
                ..
            }) => return Ok(()),
            Some(VmEvent::UnexpectedExit {
                cause: VmExitCause::Failure,
                detail,
            }) => {
                return Err(failure(detail));
            }
            Some(VmEvent::EventsDropped { .. }) if vm.state() == VmState::Failed => {
                return Err(failure("guest exit details were unavailable".into()));
            }
            None => {
                return Err(failure(
                    "VM lifecycle event stream closed during shutdown".into(),
                ));
            }
            _ => {}
        }
    }
}

pub(super) async fn guest_exit(vm: &dyn Vm, events: &mut VmEventStream) -> String {
    loop {
        match events.next().await {
            Some(VmEvent::UnexpectedExit { detail, .. }) => return detail,
            Some(VmEvent::StateChanged {
                current: VmState::Stopped,
                ..
            }) => return "guest stopped".into(),
            Some(VmEvent::EventsDropped { .. })
                if matches!(vm.state(), VmState::Failed | VmState::Stopped) =>
            {
                return "guest stopped while events were unavailable".into();
            }
            None => return "VM lifecycle event stream closed".into(),
            _ => {}
        }
    }
}
