//! Keep ownership of a partial VM while timing out or canceling individual boot phases.

use std::{future::Future, io, sync::Arc, time::Duration};

use marsa_core::settings::NetworkMode;
use marsa_proto::guest::v1::ConfigureNetwork;
use marsa_vm::{Hypervisor, Vm, VmError, VmEvent, VmEventStream, VmExitCause, VmSpec, VmState};
use tokio::{
    sync::{oneshot, watch},
    time::{Instant, timeout, timeout_at},
};
use tokio_stream::StreamExt;

use super::{
    AlternateFailure, BootNetwork, BootStage, GuestProbe, GuestSession, NetworkResult,
    launch::{Applied, Launch, Source},
    network::Selection,
    platform,
};

mod selection;

pub(super) struct LiveGuest {
    pub vm: Box<dyn Vm>,
    pub session: Option<Box<dyn GuestSession>>,
    pub events: VmEventStream,
    pub network: BootNetwork,
    pub configured_mode: Option<NetworkMode>,
    pub applied: Option<Applied>,
    network_lost: bool,
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
    pub(super) deadline: Instant,
    pub(super) stop_limit: Duration,
    pub(super) selection: Selection,
    pub(super) network_policy: Option<ConfigureNetwork>,
}

pub(super) async fn run(
    source: Source,
    probe: Arc<dyn GuestProbe>,
    limit: Duration,
    stop_limit: Duration,
    mut cancel: oneshot::Receiver<()>,
    progress: watch::Sender<BootStage>,
) -> Outcome {
    let deadline = Instant::now() + limit;
    let launch = match source {
        Source::Fixed(launch) => *launch,
        Source::Package(prepare) => {
            progress.send_replace(BootStage::Prepare);
            match prepare_package(prepare, deadline, &mut cancel).await {
                Ok(launch) => launch,
                Err(issue) => return Outcome::Failed { vm: None, issue },
            }
        }
    };
    let console = launch.spec.console_log.clone();
    let configured_mode = launch.selection.configured_mode;
    let console_position = platform::console_position(&console).await;
    let result = selection::select_and_start(
        launch.backend.as_ref(),
        probe.as_ref(),
        Activation {
            spec: launch.spec,
            deadline,
            stop_limit,
            selection: launch.selection,
            network_policy: launch.network_policy,
        },
        &mut cancel,
        &progress,
    )
    .await;
    match result {
        Ok(mut guest) => {
            guest.applied = launch.applied;
            guest.configured_mode = configured_mode;
            Outcome::Ready(guest)
        }
        Err(mut failure) => {
            if failure.issue.stage == BootStage::Handshake
                && !matches!(failure.issue.reason, Reason::Canceled)
                && let Some(schema) = platform::disk_schema_error(console, console_position).await
            {
                failure.issue = Issue::failed(
                    BootStage::DataDisk,
                    format!(
                        "Data disk schema {} requires a newer release of Marsa (this release supports {}). Use a compatible release to open the disk. | يتطلب تنسيق قرص البيانات {} إصدارًا أحدث من مارسا (يدعم هذا الإصدار {}). استخدم إصدارًا متوافقًا لفتح القرص.",
                        schema.found, schema.supported, schema.found, schema.supported,
                    ),
                );
            }
            failure.into()
        }
    }
}

async fn prepare_package(
    prepare: super::launch::Prepare,
    deadline: Instant,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Launch, Issue> {
    let mut worker = tokio::task::spawn_blocking(move || prepare());
    let result = phase(BootStage::Prepare, deadline, cancel, async {
        (&mut worker).await.map_err(io::Error::other)?
    })
    .await;
    if result.is_err() && !worker.is_finished() {
        // A started file operation cannot be cancelled. Keep the activation
        // serialized until it finishes before allowing a new preparation.
        worker.abort();
        let _ = worker.await;
    }
    result
}

async fn attach(
    backend: &dyn Hypervisor,
    probe: &dyn GuestProbe,
    spec: VmSpec,
    network_policy: Option<&ConfigureNetwork>,
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
    let mut network_lost = false;
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
        during_guest_execution(
            vm.as_ref(),
            &mut events,
            &mut network_lost,
            probe.handshake_for_boot(vm.as_ref(), network_policy),
        ),
    )
    .await;
    match session {
        Ok(session) => Ok(LiveGuest {
            vm,
            session: Some(session),
            events,
            network_lost,
            applied: None,
            configured_mode: None,
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
            &mut guest.network_lost,
            session.engine_ready(guest.vm.as_ref()),
        ),
    )
    .await;
    match ready {
        Ok(()) => {
            if guest.network_lost {
                guest.network.result = match guest.network.result {
                    NetworkResult::Fallback(native) => NetworkResult::AlternateFailed {
                        native,
                        alternate: AlternateFailure::Transport,
                    },
                    _ => NetworkResult::TransportUnavailable,
                };
            }
            Ok(guest)
        }
        Err(issue) => Err(Failed {
            vm: Some(guest.vm),
            issue,
        }),
    }
}

pub(super) async fn ensure_stopped(vm: &mut dyn Vm, limit: Duration) -> Result<(), VmError> {
    // A backend can accept start before publishing Starting. Always order a
    // stop after any submitted work, even if the observed state is Created.
    let completion = limit.saturating_add(Duration::from_secs(5));
    timeout(completion, vm.stop(limit))
        .await
        .map_err(|_| VmError::Timeout {
            operation: "stop",
            limit: completion,
        })??;
    if vm.state() != VmState::Stopped {
        return Err(VmError::Backend {
            operation: "stop",
            detail: "guest did not confirm shutdown".into(),
        });
    }
    Ok(())
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
        result = async {
            if Instant::now() >= deadline {
                return Err(Issue { stage, reason: Reason::Timeout });
            }
            match timeout_at(deadline, work).await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(Issue::failed(stage, error.to_string())),
                Err(_) => Err(Issue { stage, reason: Reason::Timeout }),
            }
        } => result,
    }
}

async fn during_guest_execution<T>(
    vm: &dyn Vm,
    events: &mut VmEventStream,
    network_lost: &mut bool,
    work: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::select! {
        biased;
        detail = guest_exit_observing(vm, events, || *network_lost = true) => Err(io::Error::other(detail)),
        result = work => result,
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
    guest_exit_observing(vm, events, || {}).await
}

async fn guest_exit_observing(
    vm: &dyn Vm,
    events: &mut VmEventStream,
    mut transport_lost: impl FnMut(),
) -> String {
    loop {
        match events.next().await {
            Some(VmEvent::UnexpectedExit { detail, .. }) => return detail,
            Some(VmEvent::NetworkUnavailable { .. }) => transport_lost(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, mpsc};

    #[tokio::test]
    async fn cancelling_preparation_waits_for_the_in_flight_file_operation() {
        let (starting, started) = oneshot::channel();
        let (release, waiting) = mpsc::channel();
        let input = Mutex::new(Some((starting, waiting)));
        let (cancel, mut canceled) = oneshot::channel();
        let worker = tokio::spawn(async move {
            prepare_package(
                Arc::new(move || {
                    let (starting, waiting) = input.lock().unwrap().take().unwrap();
                    starting.send(()).unwrap();
                    waiting.recv_timeout(Duration::from_secs(2)).unwrap();
                    Err(io::ErrorKind::InvalidData.into())
                }),
                Instant::now() + Duration::from_secs(2),
                &mut canceled,
            )
            .await
        });
        started.await.unwrap();
        cancel.send(()).unwrap();
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        release.send(()).unwrap();
        assert!(matches!(
            worker.await.unwrap(),
            Err(Issue {
                stage: BootStage::Prepare,
                reason: Reason::Canceled,
            })
        ));
    }
}
