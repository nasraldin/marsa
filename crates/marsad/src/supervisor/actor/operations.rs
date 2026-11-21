//! Implement the actor's state transitions for requests, deadlines, and boot completion.

use std::{sync::Arc, time::Duration};

use marsa_vm::Vm;
use tokio::{
    task::JoinError,
    time::{Instant, timeout},
};

use crate::supervisor::{
    BootFailure, BootStage, State, SupervisorError,
    boot::{self, Issue, LiveGuest, Outcome, Reason},
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
            Command::HostWillSleep(reply) => (reply, self.host_will_sleep().await),
            Command::HostWoke(reply) => (reply, self.host_woke().await),
            Command::IdlePolicy(stop, timeout, reply) => {
                self.config.idle_stop = stop;
                self.config.idle_timeout = timeout;
                self.reset_idle();
                let result = if stop && self.state() == State::Idle {
                    self.stop().await
                } else {
                    Ok(())
                };
                (reply, result)
            }
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
            State::Failed if self.retry_at.is_none() && !self.boot_after_wake => {
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
                if state == State::Stopped && !self.boot_after_wake {
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

    async fn host_will_sleep(&mut self) -> Result<(), SupervisorError> {
        self.host_asleep = true;
        if !self.runtime.pause_resume {
            return Ok(());
        }
        if let Some(outcome) = self.cancel_start().await {
            return match outcome {
                Ok(Outcome::Ready(guest)) => self.pause_started_guest(guest, true).await,
                Ok(Outcome::Interrupted(guest)) => self.pause_started_guest(guest, false).await,
                Ok(Outcome::Failed { vm, issue }) if matches!(issue.reason, Reason::Canceled) => {
                    self.defer_incomplete_boot(vm).await
                }
                outcome => {
                    self.boot_finished(outcome).await;
                    Ok(())
                }
            };
        }
        if matches!(self.state(), State::Ready | State::Idle) {
            self.pause().await?;
            self.paused_for_sleep = true;
        }
        Ok(())
    }

    async fn pause_started_guest(
        &mut self,
        guest: LiveGuest,
        ready: bool,
    ) -> Result<(), SupervisorError> {
        self.booting();
        self.guest = Some(guest);
        match self.pause_guest().await {
            Ok(()) => {
                self.paused_for_sleep = true;
                Ok(())
            }
            Err(error) => {
                if ready {
                    self.ready();
                } else if let Some(guest) = &mut self.guest {
                    // Retain a live VM even if native pause failed. Wake will
                    // negotiate a fresh control session and complete readiness.
                    guest.session.take();
                }
                Err(error)
            }
        }
    }

    async fn defer_incomplete_boot(
        &mut self,
        vm: Option<Box<dyn Vm>>,
    ) -> Result<(), SupervisorError> {
        self.transition(State::Stopping);
        if let Err(error) = self.stop_vm(vm, self.config.stop_timeout).await {
            self.record_shutdown_failure(&error).await;
            self.reply_waiters(Err(error.clone()));
            return Err(error);
        }
        self.boot_after_wake = true;
        self.transition(State::Stopped);
        Ok(())
    }

    async fn host_woke(&mut self) -> Result<(), SupervisorError> {
        self.host_asleep = false;
        if std::mem::take(&mut self.boot_after_wake) {
            self.begin();
        } else if self.paused_for_sleep && self.state() == State::Paused {
            self.resume().await?;
        } else if self.state() == State::Booting && self.guest.is_some() {
            self.restore_readiness(false).await?;
        } else if matches!(self.state(), State::Ready | State::Idle)
            && let Some(session) = self.guest.as_mut().and_then(|guest| guest.session.as_mut())
        {
            session
                .synchronize_clock()
                .await
                .map_err(|error| SupervisorError::Clock(Arc::new(error)))?;
        }
        Ok(())
    }

    async fn pause(&mut self) -> Result<(), SupervisorError> {
        if !matches!(self.state(), State::Ready | State::Idle) {
            return Err(self.invalid("pause"));
        }
        self.pause_guest().await
    }

    async fn pause_guest(&mut self) -> Result<(), SupervisorError> {
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
        self.paused_for_sleep = false;
        self.restore_readiness(true).await
    }

    async fn restore_readiness(&mut self, was_paused: bool) -> Result<(), SupervisorError> {
        let guest = self.guest.as_mut().ok_or(SupervisorError::Closed)?;
        let result = timeout(self.config.boot_timeout, async {
            if was_paused {
                guest.vm.resume().await.map_err(std::io::Error::other)?;
            }
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

    async fn cancel_start(&mut self) -> Option<Result<Outcome, JoinError>> {
        let task = self.boot.take()?;
        if let Some(cancel) = self.cancel_boot.take() {
            let _ = cancel.send(());
        }
        self.progress = None;
        Some(task.await)
    }

    pub(super) async fn stop(&mut self) -> Result<(), SupervisorError> {
        self.paused_for_sleep = false;
        self.boot_after_wake = false;
        self.retry_at = None;
        self.idle_at = None;
        if self.state() == State::Stopped {
            return Ok(());
        }
        self.transition(State::Stopping);
        self.reply_waiters(Err(SupervisorError::Stopped));
        let mut ready = self.guest.take();
        let mut vm = self.cleanup.take();
        if let Some(outcome) = self.cancel_start().await {
            match outcome {
                Ok(Outcome::Ready(guest) | Outcome::Interrupted(guest)) => ready = Some(guest),
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
            self.record_shutdown_failure(&error).await;
            return Err(error);
        }
        self.transition(State::Stopped);
        Ok(())
    }

    async fn record_shutdown_failure(&mut self, error: &SupervisorError) {
        let failure = Arc::new(BootFailure {
            stage: BootStage::Shutdown,
            timed_out: false,
            detail: error.to_string(),
            console_tail: platform::console_tail(self.runtime.console_log.clone()).await,
        });
        self.set_state(State::Failed, Some(failure));
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
        self.boot = None;
        self.cancel_boot = None;
        self.progress = None;
        match outcome {
            Ok(Outcome::Ready(guest)) => {
                self.booting();
                self.guest = Some(guest);
                self.ready();
            }
            Ok(Outcome::Interrupted(guest)) => {
                self.failed(
                    Some(guest.vm),
                    Issue {
                        stage: BootStage::Engine,
                        reason: Reason::Canceled,
                    },
                )
                .await;
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
