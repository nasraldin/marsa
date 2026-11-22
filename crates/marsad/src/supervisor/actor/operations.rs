//! Implement the actor's state transitions for requests, deadlines, and boot completion.

use std::{io, sync::Arc, time::Duration};

use marsa_proto::guest::DOCKER_API_PORT;
use marsa_vm::{Vm, VsockStream};
use tokio::{
    sync::oneshot,
    task::JoinError,
    time::{Instant, timeout},
};

use crate::supervisor::{
    BootFailure, BootStage, State, SupervisorError,
    boot::{self, Issue, Outcome, Reason},
    platform,
};

use super::{Actor, CHANNEL_CAPACITY, Command, Reply};

impl Actor {
    pub(super) async fn command(&mut self, command: Command) {
        let (reply, result) = match command {
            Command::Reserve {
                expected,
                release,
                reply,
            } => {
                self.reserve_maintenance(expected, release, reply);
                return;
            }
            Command::MaintenanceStart(id, reply) => {
                self.maintenance_start(id, reply);
                return;
            }
            Command::MaintenanceStop(id, reply) => {
                let result = if self.maintenance_owns(id) && !reply.is_closed() {
                    self.stop().await
                } else {
                    Err(SupervisorError::Busy)
                };
                (reply, result)
            }
            Command::EnsureReady(reply)
            | Command::Stop(reply)
            | Command::Restart(reply)
            | Command::Pause(reply)
            | Command::Resume(reply)
                if self.maintenance.is_some() =>
            {
                (reply, Err(SupervisorError::Busy))
            }
            Command::EnsureReady(reply) => {
                self.ensure_ready(reply);
                return;
            }
            Command::ConnectDocker(reply) => {
                self.connect_docker(reply).await;
                return;
            }
            Command::Stop(reply) => (reply, self.stop().await),
            Command::Restart(reply) => {
                if let Err(error) = self.stop().await {
                    let _ = reply.send(Err(error));
                } else {
                    self.ensure_ready(reply);
                }
                return;
            }
            Command::Pause(reply) => (reply, self.pause().await),
            Command::Resume(reply) => (reply, self.resume().await),
            Command::IdlePolicy(stop, timeout, reply) => {
                self.config.idle_stop = stop;
                self.config.idle_timeout = timeout;
                self.reset_idle();
                if self.maintenance.is_none()
                    && stop
                    && self.state() == State::Idle
                    && self.empty
                    && self.docker_quiet()
                {
                    self.idle_at = Some(Instant::now());
                }
                (reply, Ok(()))
            }
            Command::ContainerCount(count, reply) => {
                self.container_count(count);
                (reply, Ok(()))
            }
        };
        let _ = reply.send(result);
    }

    async fn connect_docker(&mut self, mut reply: oneshot::Sender<io::Result<VsockStream>>) {
        if self.maintenance.is_some() {
            let _ = reply.send(Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                SupervisorError::Busy,
            )));
            return;
        }
        let result = match self.guest.as_ref().map(|guest| guest.vm.as_ref()) {
            Some(vm) if matches!(self.state(), State::Ready | State::Idle) => {
                tokio::select! {
                    biased;
                    () = reply.closed() => return,
                    // Keep an unresponsive backend from holding up lifecycle commands.
                    result = timeout(Duration::from_secs(2), vm.connect_vsock(DOCKER_API_PORT)) => {
                        result.map_err(io::Error::from).and_then(|stream| stream.map_err(io::Error::other))
                    }
                }
            }
            _ => Err(io::ErrorKind::NotConnected.into()),
        };
        let _ = reply.send(result);
    }

    fn ensure_ready(&mut self, reply: Reply) {
        match self.state() {
            State::Ready | State::Idle => {
                self.transition(State::Ready);
                self.reset_idle();
                let _ = reply.send(Ok(()));
            }
            State::Failed if self.retry_at.is_none() => {
                let failure = self.output.snapshot.borrow().failure.clone();
                let error = failure.map_or(SupervisorError::Closed, SupervisorError::Boot);
                let _ = reply.send(Err(error));
            }
            state => {
                self.waiters.retain(|waiter| !waiter.is_closed());
                if self.waiters.len() == CHANNEL_CAPACITY {
                    let _ = reply.send(Err(SupervisorError::Busy));
                    return;
                }
                self.waiters.push(reply);
                if state == State::Stopped {
                    self.reset_retry();
                    self.begin();
                }
            }
        }
    }

    fn container_count(&mut self, count: u32) {
        self.empty = count == 0;
        if self.maintenance.is_some() {
            return;
        }
        if count > 0 {
            self.idle_at = None;
            if self.state() == State::Idle {
                self.transition(State::Ready);
            }
        } else if (self.state() == State::Ready
            || self.state() == State::Idle && self.config.idle_stop)
            && self.docker_quiet()
        {
            self.idle_at
                .get_or_insert_with(|| Instant::now() + self.config.idle_timeout);
        }
    }

    pub(super) async fn workload_sample(&mut self, count: Option<io::Result<u32>>) {
        if matches!(self.docker_clients.has_changed(), Ok(true)) {
            self.docker_activity();
            return;
        }
        self.sample_at = count
            .as_ref()
            .map(|_| Instant::now() + Duration::from_secs(2));
        if let Some(count) = count {
            // An unavailable or malformed observation never authorizes idle stop.
            self.container_count(count.unwrap_or(1));
        }
        if self.idle_at.is_some_and(|at| at <= Instant::now()) {
            self.idle().await;
        }
    }

    pub(super) async fn idle(&mut self) {
        if self.maintenance.is_some() {
            return;
        }
        if matches!(self.docker_clients.has_changed(), Ok(true)) {
            self.docker_activity();
            return;
        }
        self.idle_at = None;
        if self.empty && self.docker_quiet() && matches!(self.state(), State::Ready | State::Idle) {
            self.transition(State::Idle);
            if self.config.idle_stop {
                let _ = self.stop().await;
            }
        }
    }

    async fn pause(&mut self) -> Result<(), SupervisorError> {
        if !matches!(self.state(), State::Ready | State::Idle) {
            return Err(self.invalid("pause"));
        }
        let guest = self.guest.as_mut().ok_or(SupervisorError::Closed)?;
        guest.vm.pause().await.map_err(Arc::new)?;
        guest.session.take();
        self.idle_at = None;
        self.sample_at = None;
        self.transition(State::Paused);
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), SupervisorError> {
        if self.state() != State::Paused {
            return Err(self.invalid("resume"));
        }
        let guest = self.guest.as_mut().ok_or(SupervisorError::Closed)?;
        let result = timeout(self.config.boot_timeout, async {
            guest.vm.resume().await.map_err(std::io::Error::other)?;
            let mut session = self.probe.handshake(guest.vm.as_ref()).await?;
            session.engine_ready(guest.vm.as_ref()).await?;
            Ok::<_, std::io::Error>(session)
        })
        .await;
        let reason = match result {
            Ok(Ok(session)) => {
                guest.session = Some(session);
                self.ready();
                return Ok(());
            }
            Ok(Err(error)) => Reason::Failed(error.to_string()),
            Err(_) => Reason::Timeout,
        };
        let vm = self.guest.take().map(|guest| guest.vm);
        let failure = self
            .failed(
                vm,
                Issue {
                    stage: BootStage::Resume,
                    reason,
                },
            )
            .await;
        Err(SupervisorError::Boot(failure))
    }

    pub(super) async fn stop(&mut self) -> Result<(), SupervisorError> {
        self.retry_at = None;
        self.idle_at = None;
        self.sample_at = None;
        if self.state() == State::Stopped {
            return Ok(());
        }
        self.transition(State::Stopping);
        self.reply_waiters(Err(SupervisorError::Stopped));
        if let Some(cancel) = self.cancel_boot.take() {
            let _ = cancel.send(());
        }
        self.progress = None;
        let mut ready = self.guest.take();
        let mut vm = self.cleanup.take();
        if let Some(task) = self.boot.take() {
            match task.await {
                Ok(Outcome::Ready(guest)) => ready = Some(guest),
                Ok(Outcome::Failed { vm: partial, .. }) => vm = partial,
                Err(_) => {}
            }
        }
        let mut limit = self.config.stop_timeout;
        let mut failure = None;
        if let Some(guest) = ready {
            let (running, remaining, exit_failure) =
                boot::graceful_stop(guest, self.probe.as_ref(), limit).await;
            vm = Some(running);
            limit = remaining;
            failure = exit_failure;
        }
        let outcome = self.stop_vm(vm, limit).await.and_then(|()| {
            failure.map_or(Ok(()), |error| Err(SupervisorError::Vm(Arc::new(error))))
        });
        if let Err(error) = outcome {
            let failure = Arc::new(BootFailure {
                stage: BootStage::Shutdown,
                timed_out: false,
                detail: error.to_string(),
                console_tail: platform::console_tail(self.runtime.console_log.clone()).await,
            });
            self.set_state(State::Failed, Some(failure));
            return Err(error);
        }
        self.transition(State::Stopped);
        Ok(())
    }

    async fn stop_vm(
        &mut self,
        vm: Option<Box<dyn Vm>>,
        limit: Duration,
    ) -> Result<(), SupervisorError> {
        self.cleanup = vm;
        if let Some(vm) = self.cleanup.as_deref_mut() {
            boot::ensure_stopped(vm, limit).await.map_err(Arc::new)?;
        }
        self.cleanup = None;
        Ok(())
    }

    pub(super) async fn boot_finished(&mut self, outcome: Result<Outcome, JoinError>) {
        self.maintenance_boot_finished();
        self.boot = None;
        self.cancel_boot = None;
        self.progress = None;
        match outcome {
            Ok(Outcome::Ready(mut guest)) => {
                if let Some(applied) = guest.applied.take() {
                    applied();
                }
                self.booting();
                self.guest = Some(guest);
                self.ready();
            }
            Ok(Outcome::Failed { vm, issue }) => {
                if matches!(
                    issue.stage,
                    BootStage::Handshake | BootStage::Network | BootStage::Engine
                ) {
                    self.booting();
                }
                self.failed(vm, issue).await;
            }
            Err(error) => {
                self.failed(None, Issue::failed(BootStage::Start, error.to_string()))
                    .await;
            }
        }
    }

    pub(super) async fn failed(
        &mut self,
        vm: Option<Box<dyn Vm>>,
        issue: Issue,
    ) -> Arc<BootFailure> {
        let (timed_out, mut detail) = match issue.reason {
            Reason::Timeout => (true, "guest readiness deadline reached".into()),
            Reason::Failed(detail) => (false, detail),
            Reason::Canceled => (false, "guest activation canceled".into()),
        };
        self.idle_at = None;
        self.sample_at = None;
        let stop = self.stop_vm(vm, self.config.stop_timeout).await;
        if let Err(error) = &stop {
            detail.push_str(&format!("; guest cleanup failed: {error}"));
        }
        let failure = Arc::new(BootFailure {
            stage: issue.stage,
            timed_out,
            detail,
            console_tail: if issue.stage == BootStage::Prepare {
                String::new()
            } else {
                platform::console_tail(self.runtime.console_log.clone()).await
            },
        });
        tracing::warn!(
            stage = ?failure.stage,
            timed_out = failure.timed_out,
            "guest activation failed"
        );
        self.set_state(State::Failed, Some(failure.clone()));
        if self.retries_left > 0 && stop.is_ok() && issue.stage != BootStage::Prepare {
            self.retries_left -= 1;
            self.retry_at = Some(Instant::now() + self.backoff);
            self.backoff = self
                .backoff
                .saturating_mul(2)
                .min(self.config.maximum_backoff);
        } else {
            if stop.is_ok() {
                self.transition(State::Stopped);
            }
            self.reply_waiters(Err(SupervisorError::Boot(failure.clone())));
        }
        failure
    }

    fn invalid(&self, operation: &'static str) -> SupervisorError {
        SupervisorError::InvalidState {
            operation,
            state: self.state(),
        }
    }
}
