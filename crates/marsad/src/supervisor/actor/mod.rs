//! Serialize lifecycle decisions while VM startup and guest readiness run independently.

use std::{future::pending, sync::Arc, time::Duration};

use marsa_vm::{Hypervisor, Vm, VmSpec};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::{JoinError, JoinHandle},
    time::{Instant, sleep_until},
};

use super::{
    BootFailure, BootStage, GuestProbe, Snapshot, State, SupervisorConfig, SupervisorError,
    Transition,
    boot::{self, LiveGuest, Outcome},
};

mod operations;

pub(super) const CHANNEL_CAPACITY: usize = 64;
pub(super) type Reply = oneshot::Sender<Result<(), SupervisorError>>;

pub(super) enum Command {
    EnsureReady(Reply),
    Stop(Reply),
    Pause(Reply),
    Resume(Reply),
    ContainerCount(u32, Reply),
}

enum Event {
    Command(Option<Command>),
    Progress(BootStage),
    Boot(Result<Outcome, JoinError>),
    GuestExited(String),
    Retry,
    Idle,
}

pub(super) struct Actor {
    backend: Arc<dyn Hypervisor>,
    probe: Arc<dyn GuestProbe>,
    spec: VmSpec,
    config: SupervisorConfig,
    commands: mpsc::Receiver<Command>,
    snapshot: watch::Sender<Snapshot>,
    events: broadcast::Sender<Transition>,
    boot: Option<JoinHandle<Outcome>>,
    progress: Option<watch::Receiver<BootStage>>,
    cancel_boot: Option<oneshot::Sender<()>>,
    guest: Option<LiveGuest>,
    cleanup: Option<Box<dyn Vm>>,
    waiters: Vec<Reply>,
    retry_at: Option<Instant>,
    retries_left: u32,
    backoff: Duration,
    idle_at: Option<Instant>,
    empty: bool,
}

impl Actor {
    pub(super) fn new(
        backend: Arc<dyn Hypervisor>,
        probe: Arc<dyn GuestProbe>,
        spec: VmSpec,
        config: SupervisorConfig,
        commands: mpsc::Receiver<Command>,
        snapshot: watch::Sender<Snapshot>,
        events: broadcast::Sender<Transition>,
    ) -> Self {
        Self {
            backend,
            probe,
            spec,
            config,
            commands,
            snapshot,
            events,
            boot: None,
            progress: None,
            cancel_boot: None,
            guest: None,
            cleanup: None,
            waiters: Vec::new(),
            retry_at: None,
            retries_left: 0,
            backoff: Duration::ZERO,
            idle_at: None,
            empty: false,
        }
    }

    pub(super) async fn run(mut self) {
        loop {
            let event = tokio::select! {
                command = self.commands.recv() => Event::Command(command),
                stage = watch_progress(&mut self.progress) => Event::Progress(stage),
                outcome = wait_boot(&mut self.boot) => Event::Boot(outcome),
                detail = watch_guest(&mut self.guest) => Event::GuestExited(detail),
                () = wait_deadline(self.retry_at) => Event::Retry,
                () = wait_deadline(self.idle_at) => Event::Idle,
            };
            match event {
                Event::Command(Some(command)) => self.command(command).await,
                Event::Command(None) => {
                    let _ = self.stop().await;
                    return;
                }
                Event::Progress(BootStage::Handshake | BootStage::Engine) => self.booting(),
                Event::Progress(_) => {}
                Event::Boot(outcome) => self.boot_finished(outcome).await,
                Event::GuestExited(detail) => {
                    let vm = self.guest.take().map(|guest| guest.vm);
                    self.failed(vm, boot::Issue::failed(BootStage::Running, detail))
                        .await;
                }
                Event::Retry => self.begin(),
                Event::Idle => self.idle().await,
            }
        }
    }

    fn state(&self) -> State {
        self.snapshot.borrow().state
    }

    fn set_state(&self, current: State, failure: Option<Arc<BootFailure>>) {
        let previous = self.state();
        if previous != current {
            self.snapshot.send_replace(Snapshot {
                state: current,
                failure: failure.clone(),
            });
            let _ = self.events.send(Transition {
                previous,
                current,
                failure,
            });
        }
    }

    fn transition(&self, current: State) {
        let failure = self.snapshot.borrow().failure.clone();
        self.set_state(current, failure);
    }

    fn reply_waiters(&mut self, result: Result<(), SupervisorError>) {
        for waiter in self.waiters.drain(..) {
            let _ = waiter.send(result.clone());
        }
    }

    fn reset_retry(&mut self) {
        self.retries_left = self.config.max_retries;
        self.backoff = self.config.initial_backoff.min(self.config.maximum_backoff);
    }

    fn begin(&mut self) {
        let (cancel_boot, cancel) = oneshot::channel();
        let (progress, updates) = watch::channel(BootStage::Create);
        self.retry_at = None;
        self.idle_at = None;
        self.empty = false;
        self.transition(State::Starting);
        self.cancel_boot = Some(cancel_boot);
        self.progress = Some(updates);
        self.boot = Some(tokio::spawn(boot::run(
            self.backend.clone(),
            self.probe.clone(),
            self.spec.clone(),
            self.config.boot_timeout,
            cancel,
            progress,
        )));
    }

    fn booting(&self) {
        if self.state() == State::Starting {
            self.transition(State::Booting);
        }
    }

    fn ready(&mut self) {
        self.set_state(State::Ready, None);
        self.reset_idle();
        self.reply_waiters(Ok(()));
    }

    fn reset_idle(&mut self) {
        self.idle_at = (self.empty && self.state() == State::Ready)
            .then(|| Instant::now() + self.config.idle_timeout);
    }
}

async fn wait_boot(boot: &mut Option<JoinHandle<Outcome>>) -> Result<Outcome, JoinError> {
    match boot {
        Some(task) => task.await,
        None => pending().await,
    }
}

async fn watch_progress(progress: &mut Option<watch::Receiver<BootStage>>) -> BootStage {
    if let Some(updates) = progress
        && updates.changed().await.is_ok()
    {
        return *updates.borrow_and_update();
    }
    *progress = None;
    pending().await
}

async fn watch_guest(guest: &mut Option<LiveGuest>) -> String {
    match guest {
        Some(guest) => boot::guest_exit(guest.vm.as_ref(), &mut guest.events).await,
        None => pending().await,
    }
}

async fn wait_deadline(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        sleep_until(deadline).await;
    } else {
        pending().await
    }
}
