//! Serialize lifecycle decisions while VM startup and guest readiness run independently.

use std::{future::pending, io, sync::Arc, time::Duration};

use marsa_core::settings::NetworkMode;
use marsa_vm::{Vm, VsockStream};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::{JoinError, JoinHandle},
    time::{Instant, sleep_until},
};

use super::{
    BootFailure, BootStage, GuestProbe, NetworkObservation, Observer, Snapshot, State,
    SupervisorConfig, SupervisorError, Transition,
    boot::{self, LiveGuest, Outcome},
    launch::Runtime,
};

mod maintenance;
mod operations;

pub(super) const CHANNEL_CAPACITY: usize = 64;
pub(super) type Reply = oneshot::Sender<Result<(), SupervisorError>>;
pub(super) type ReservationReply = oneshot::Sender<Result<u64, SupervisorError>>;

pub(super) enum Command {
    Reserve {
        expected: Option<u64>,
        release: oneshot::Receiver<()>,
        reply: ReservationReply,
    },
    MaintenanceStart(u64, bool, Reply),
    MaintenanceKeep(u64, Reply),
    MaintenanceStop(u64, Reply),
    EnsureReady(Reply),
    ConnectDocker(oneshot::Sender<io::Result<VsockStream>>),
    Network(NetworkMode, oneshot::Sender<NetworkObservation>),
    Stop(Reply),
    Restart(Reply),
    Pause(Reply),
    Resume(Reply),
    HostWillSleep(Reply),
    HostWoke(Reply),
    ContainerCount(u32, Reply),
    IdlePolicy(bool, Duration, Reply),
}

enum Event {
    Command(Option<Command>),
    Progress(BootStage),
    Boot(Result<Outcome, JoinError>),
    GuestExited(String),
    DockerClients,
    ReleaseMaintenance,
    Workloads(Option<io::Result<u32>>),
    Retry,
    Idle,
}

pub(super) struct TransitionOutput {
    pub(super) snapshot: watch::Sender<Snapshot>,
    pub(super) events: broadcast::Sender<Transition>,
    pub(super) observer: Option<Observer>,
}

pub(super) struct Actor {
    runtime: Runtime,
    probe: Arc<dyn GuestProbe>,
    config: SupervisorConfig,
    commands: mpsc::Receiver<Command>,
    docker_clients: watch::Receiver<usize>,
    output: TransitionOutput,
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
    sample_at: Option<Instant>,
    empty: bool,
<<<<<<< HEAD
    maintenance: Option<maintenance::Reservation>,
=======
<<<<<<< HEAD
    paused_for_sleep: bool,
    host_asleep: bool,
    boot_after_wake: bool,
=======
    maintenance: Option<maintenance::Reservation>,
>>>>>>> origin/main
>>>>>>> origin/main
}

impl Actor {
    pub(super) fn new(
        runtime: Runtime,
        probe: Arc<dyn GuestProbe>,
        config: SupervisorConfig,
        commands: mpsc::Receiver<Command>,
        docker_clients: watch::Receiver<usize>,
        output: TransitionOutput,
    ) -> Self {
        Self {
            runtime,
            probe,
            config,
            commands,
            docker_clients,
            output,
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
            sample_at: None,
            empty: false,
<<<<<<< HEAD
            maintenance: None,
=======
<<<<<<< HEAD
            paused_for_sleep: false,
            host_asleep: false,
            boot_after_wake: false,
=======
            maintenance: None,
>>>>>>> origin/main
>>>>>>> origin/main
        }
    }

    pub(super) async fn run(mut self) {
        loop {
            if self.maintenance_released() {
                self.release_maintenance().await;
                continue;
            }
            let sample = self
                .sample_at
                .map(|at| self.idle_at.map_or(at, |idle| idle.min(at)))
                .filter(|_| self.maintenance.is_none() && self.docker_quiet());
            let manual_idle = self
                .idle_at
                .filter(|_| self.maintenance.is_none() && self.sample_at.is_none());
            let ordinary = async {
                tokio::select! {
                    command = self.commands.recv() => Event::Command(command),
                    stage = watch_progress(&mut self.progress) => Event::Progress(stage),
                    outcome = wait_boot(&mut self.boot) => Event::Boot(outcome),
                    guest = watch_guest(&mut self.guest, self.probe.as_ref(), sample) => guest,
                    Ok(()) = self.docker_clients.changed() => Event::DockerClients,
                    () = wait_deadline(self.retry_at) => Event::Retry,
                    () = wait_deadline(manual_idle) => Event::Idle,
                }
            };
            let event = tokio::select! {
                biased;
                () = maintenance::wait_release(&mut self.maintenance) => Event::ReleaseMaintenance,
                event = ordinary => event,
            };
            match event {
                Event::Command(Some(command)) => {
                    if self.maintenance_released() {
                        self.release_maintenance().await;
                    }
                    self.command(command).await;
                }
                Event::ReleaseMaintenance => self.release_maintenance().await,
                Event::Command(None) => {
                    let _ = self.stop().await;
                    return;
                }
                Event::Progress(BootStage::Handshake | BootStage::Network | BootStage::Engine) => {
                    self.booting()
                }
                Event::Progress(_) => {}
                Event::Boot(outcome) => self.boot_finished(outcome).await,
                Event::GuestExited(detail) => {
                    let vm = self.guest.take().map(|guest| guest.vm);
                    self.failed(vm, boot::Issue::failed(BootStage::Running, detail))
                        .await;
                }
                Event::DockerClients => self.docker_activity(),
                Event::Workloads(count) => self.workload_sample(count).await,
                Event::Retry => self.begin(),
                Event::Idle => self.idle().await,
            }
        }
    }

    fn state(&self) -> State {
        self.output.snapshot.borrow().state
    }

    fn set_state(&self, current: State, failure: Option<Arc<BootFailure>>) {
        let previous = self.state();
        if previous != current {
            let network = self.guest.as_ref().map(|guest| guest.network);
            let lifecycle_revision = self
                .output
                .snapshot
                .borrow()
                .lifecycle_revision
                .saturating_add(1);
            self.output.snapshot.send_replace(Snapshot {
                lifecycle_revision,
                state: current,
                failure: failure.clone(),
                network,
            });
            let transition = Transition {
                previous,
                current,
                failure,
                network,
            };
            if let Some(observer) = &self.output.observer {
                observer(&transition);
            }
            let _ = self.output.events.send(transition);
        }
    }

    fn bump_revision(&self) {
        self.output.snapshot.send_modify(|value| {
            value.lifecycle_revision = value.lifecycle_revision.saturating_add(1);
        });
    }

    fn transition(&self, current: State) {
        let failure = self.output.snapshot.borrow().failure.clone();
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
        self.retry_at = None;
        if self.host_asleep {
            self.boot_after_wake = true;
            return;
        }
        let (cancel_boot, cancel) = oneshot::channel();
        let (progress, updates) = watch::channel(BootStage::Create);
        self.idle_at = None;
        self.sample_at = None;
        self.empty = false;
        self.transition(State::Starting);
        self.cancel_boot = Some(cancel_boot);
        self.progress = Some(updates);
        self.boot = Some(tokio::spawn(boot::run(
            self.runtime.source.clone(),
            self.probe.clone(),
            self.config.boot_timeout,
            self.config.stop_timeout,
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
        if let Some(applied) = self.guest.as_mut().and_then(|guest| guest.applied.take()) {
            applied();
        }

        self.set_state(State::Ready, None);
        self.sample_at = Some(Instant::now());
        self.reset_idle();
        self.reply_waiters(Ok(()));
    }

    fn reset_idle(&mut self) {
        self.idle_at = (self.maintenance.is_none()
            && self.empty
            && self.docker_quiet()
            && (self.state() == State::Ready
                || self.state() == State::Idle && self.config.idle_stop))
            .then(|| Instant::now() + self.config.idle_timeout);
    }

    fn docker_quiet(&self) -> bool {
        *self.docker_clients.borrow() == 0
    }

    fn docker_activity(&mut self) {
        self.docker_clients.borrow_and_update();
        if self.docker_quiet() && self.sample_at.is_some() {
            // Activity may have started a container on the just-closed connection.
            self.empty = false;
            self.sample_at = Some(Instant::now());
        }
        self.reset_idle();
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

async fn watch_guest(
    guest: &mut Option<LiveGuest>,
    probe: &dyn GuestProbe,
    sample: Option<Instant>,
) -> Event {
    match guest {
        Some(guest) => {
            let vm = guest.vm.as_ref();
            tokio::select! {
                detail = boot::guest_exit(vm, &mut guest.events) => Event::GuestExited(detail),
                error = async {
                    match guest.session.as_mut() {
                        Some(session) => session.disconnected().await,
                        None => pending().await,
                    }
                } => Event::GuestExited(error.to_string()),
                count = async {
                    wait_deadline(sample).await;
                    probe.active_containers(vm).await
                } => Event::Workloads(count),
            }
        }
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
