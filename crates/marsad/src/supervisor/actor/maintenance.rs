//! Serialize maintenance reservation, owner actions and cancellation in the actor.

use std::future::pending;
use tokio::{sync::oneshot, time::Instant};

use super::{Actor, Reply, ReservationReply, State, SupervisorError};

pub(super) struct Reservation {
    id: u64,
    release: oneshot::Receiver<()>,
    starting: bool,
}

impl Actor {
    pub(super) fn reserve_maintenance(
        &mut self,
        expected: Option<u64>,
        release: oneshot::Receiver<()>,
        reply: ReservationReply,
    ) {
        if reply.is_closed() {
            return;
        }
        if self.maintenance.is_some() {
            let _ = reply.send(Err(SupervisorError::Busy));
            return;
        }
        let actual = self.output.snapshot.borrow().lifecycle_revision;
        if let Some(expected) = expected
            && expected != actual
        {
            let _ = reply.send(Err(SupervisorError::StaleRevision { expected, actual }));
            return;
        }
        let Some(id) = actual.checked_add(1) else {
            let _ = reply.send(Err(SupervisorError::Closed));
            return;
        };
        self.maintenance = Some(Reservation {
            id,
            release,
            starting: false,
        });
        self.bump_revision();
        // Publish the revision before handing out the permit. If the caller
        // already canceled, leave the pending engine work and timers intact.
        if reply.send(Ok(id)).is_err() {
            self.maintenance.take();
            self.bump_revision();
            return;
        }
        self.retries_left = 0;
        self.idle_at = None;
        self.reply_waiters(Err(SupervisorError::Busy));
        if self.retry_at.take().is_some() {
            // A scheduled retry has already cleaned up the failed attempt.
            self.transition(State::Stopped);
        }
    }

    pub(super) fn maintenance_owns(&self, id: u64) -> bool {
        self.maintenance
            .as_ref()
            .is_some_and(|owner| owner.id == id)
    }

    pub(super) fn maintenance_start(&mut self, id: u64, reply: Reply) {
        if reply.is_closed() {
            return;
        }
        if !self.maintenance_owns(id) {
            let _ = reply.send(Err(SupervisorError::Busy));
            return;
        }
        if self.state() != State::Stopped {
            let _ = reply.send(Err(SupervisorError::InvalidState {
                operation: "start a maintenance candidate",
                state: self.state(),
            }));
            return;
        }
        if let Some(owner) = &mut self.maintenance {
            owner.starting = true;
        }
        self.waiters.push(reply);
        self.begin();
    }

    pub(super) fn maintenance_boot_finished(&mut self) {
        if let Some(owner) = &mut self.maintenance {
            owner.starting = false;
        }
    }

    pub(super) fn maintenance_released(&mut self) -> bool {
        self.maintenance.as_mut().is_some_and(|owner| {
            !matches!(
                owner.release.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            )
        })
    }

    pub(super) async fn release_maintenance(&mut self) {
        if self
            .maintenance
            .as_ref()
            .is_some_and(|owner| owner.starting)
            && self.boot.is_some()
        {
            // Keep ordinary requests excluded until the partially started VM
            // has stopped or the actor holds it for a further cleanup attempt.
            let _ = self.stop().await;
        }
        self.maintenance.take();
        self.bump_revision();
        if matches!(self.state(), State::Ready | State::Idle) {
            self.empty = false;
            self.sample_at = Some(Instant::now());
        }
        self.reset_idle();
    }
}

pub(super) async fn wait_release(reservation: &mut Option<Reservation>) {
    if let Some(owner) = reservation {
        let _ = (&mut owner.release).await;
    } else {
        pending().await
    }
}
