//! Implement the actor's state transitions for requests, deadlines, and boot completion.

use std::sync::Arc;

use marsa_vm::{Vm, VmState};
use tokio::{
    task::JoinError,
    time::{Instant, timeout},
};

use crate::supervisor::{
    BootFailure, BootStage, State, SupervisorError,
    boot::{Issue, Outcome, Reason},
    platform,
};

use super::{Actor, CHANNEL_CAPACITY, Command, Reply};

impl Actor {
    pub(super) async fn command(&mut self, command: Command) {
        let (reply, result) = match command {
            Command::EnsureReady(reply) => {
                self.ensure_ready(reply);
                return;
            }
            Command::Stop(reply) => (reply, self.stop().await),
            Command::Pause(reply) => (reply, self.pause().await),
            Command::Resume(reply) => (reply, self.resume().await),
            Command::ContainerCount(count, reply) => {
                self.container_count(count);
                (reply, Ok(()))
            }
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
                let failure = self.snapshot.borrow().failure.clone();
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
        if count > 0 {
            self.idle_at = None;
            if self.state() == State::Idle {
                self.transition(State::Ready);
            }
        } else if self.state() == State::Ready {
            self.idle_at
                .get_or_insert_with(|| Instant::now() + self.config.idle_timeout);
        }
    }

    pub(super) async fn idle(&mut self) {
        self.idle_at = None;
        if self.state() == State::Ready {
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
        if self.state() == State::Stopped {
            return Ok(());
        }
        self.transition(State::Stopping);
        self.reply_waiters(Err(SupervisorError::Stopped));
        if let Some(cancel) = self.cancel_boot.take() {
            let _ = cancel.send(());
        }
        self.progress = None;
        let vm = if let Some(task) = self.boot.take() {
            match task.await {
                Ok(Outcome::Ready(guest)) => Some(guest.vm),
                Ok(Outcome::Failed { vm, .. }) => vm,
                Err(_) => None,
            }
        } else {
            self.cleanup
                .take()
                .or_else(|| self.guest.take().map(|guest| guest.vm))
        };
        if let Err(error) = self.stop_vm(vm).await {
            let failure = Arc::new(BootFailure {
                stage: BootStage::Shutdown,
                timed_out: false,
                detail: error.to_string(),
                console_tail: platform::console_tail(self.spec.console_log.clone()).await,
            });
            self.set_state(State::Failed, Some(failure));
            return Err(error);
        }
        self.transition(State::Stopped);
        Ok(())
    }

    async fn stop_vm(&mut self, vm: Option<Box<dyn Vm>>) -> Result<(), SupervisorError> {
        self.cleanup = vm;
        if let Some(vm) = self.cleanup.as_mut()
            && !matches!(
                vm.state(),
                VmState::Created | VmState::Failed | VmState::Stopped
            )
        {
            vm.stop(self.config.stop_timeout).await.map_err(Arc::new)?;
        }
        self.cleanup = None;
        Ok(())
    }

    pub(super) async fn boot_finished(&mut self, outcome: Result<Outcome, JoinError>) {
        self.boot = None;
        self.cancel_boot = None;
        self.progress = None;
        match outcome {
            Ok(Outcome::Ready(guest)) => {
                self.booting();
                self.guest = Some(guest);
                self.ready();
            }
            Ok(Outcome::Failed { vm, issue }) => {
                if matches!(issue.stage, BootStage::Handshake | BootStage::Engine) {
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
        let stop = self.stop_vm(vm).await;
        if let Err(error) = &stop {
            detail.push_str(&format!("; guest cleanup failed: {error}"));
        }
        let failure = Arc::new(BootFailure {
            stage: issue.stage,
            timed_out,
            detail,
            console_tail: platform::console_tail(self.spec.console_log.clone()).await,
        });
        self.set_state(State::Failed, Some(failure.clone()));
        if self.retries_left > 0 && stop.is_ok() {
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
