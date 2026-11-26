//! Recheck the active attachment without interrupting local Docker workloads.

use marsa_core::settings::NetworkMode;
use tokio::time::{Duration, Instant, timeout};

use super::{Actor, State};
use crate::supervisor::{NetworkResult, SupervisorError, Transition, network::LiveCheck};

impl Actor {
    pub(super) fn live_check(&mut self) -> Option<LiveCheck> {
        let ready = matches!(self.state(), State::Ready | State::Idle);
        let guest = self.guest.as_mut()?;
        guest.pinned_check = None;
        if !ready
            || self.maintenance.is_some()
            || self.host_asleep
            || guest.configured_mode.is_none()
            || guest.network.result == NetworkResult::TransportUnavailable
        {
            return None;
        }
        // Supersede any previous observation. Ordinary Ready/Idle changes
        // preserve the same guest, so they must not invalidate a slow probe.
        self.network_generation = self.network_generation.saturating_add(1);
        Some(LiveCheck {
            generation: self.network_generation,
            automatic: guest.configured_mode == Some(NetworkMode::Auto),
            network: guest.network,
            host: guest.live_host.clone()?,
            probe: guest.session.as_ref()?.live_egress_probe()?,
        })
    }

    pub(super) fn begin_pinned_check(&mut self) {
        if !self.guest.as_ref().is_some_and(|guest| {
            matches!(
                guest.configured_mode,
                Some(NetworkMode::Nat | NetworkMode::Userspace)
            ) && guest.network.result != NetworkResult::TransportUnavailable
        }) {
            return;
        }
        if let Some(mut check) = self.live_check() {
            if let Some(guest) = &mut self.guest {
                guest.pinned_check = Some(Box::pin(async move { check.run().await }));
            }
        } else {
            // Explicit DNS/proxy settings and older agents cannot be compared
            // with the direct host probe. Keep local availability independent.
            self.publish_network(NetworkResult::Unavailable);
        }
    }

    pub(super) fn publish_network(&mut self, result: NetworkResult) {
        let Some(guest) = self.guest.as_mut() else {
            return;
        };
        if guest.network.result == result {
            return;
        }
        self.network_generation = self.network_generation.saturating_add(1);
        guest.network.result = result;
        let network = Some(guest.network);
        self.output.snapshot.send_modify(|snapshot| {
            snapshot.network = network;
            snapshot.lifecycle_revision = snapshot.lifecycle_revision.saturating_add(1);
        });
        let snapshot = self.output.snapshot.borrow().clone();
        let event = Transition {
            previous: snapshot.state,
            current: snapshot.state,
            failure: snapshot.failure,
            network,
        };
        if let Some(observer) = &self.output.observer {
            observer(&event);
        }
        let _ = self.output.events.send(event);
    }

    pub(super) async fn live_result(
        &mut self,
        generation: u64,
        result: NetworkResult,
        allow_restart: bool,
    ) -> Result<(), SupervisorError> {
        if self.network_generation != generation
            || !matches!(self.state(), State::Ready | State::Idle)
            || self.maintenance.is_some()
            || self.host_asleep
        {
            return Ok(());
        }
        self.publish_network(result);
        if !matches!(result, NetworkResult::RestartForUserspace(_))
            || !allow_restart
            || !self.docker_quiet()
            || self
                .last_network_restart
                .is_some_and(|last| last.elapsed() < Duration::from_secs(60))
        {
            return Ok(());
        }
        let Some(guest) = self.guest.as_ref() else {
            return Ok(());
        };
        // While handling this command the actor does not admit new streams.
        // Also check the activity watch for clients that entered or left during
        // the census. Unknown inventory never authorizes a disruptive action.
        self.docker_clients.borrow_and_update();
        if !self.docker_quiet() {
            return Ok(());
        }
        let count = timeout(
            Duration::from_secs(1),
            self.probe.active_containers(guest.vm.as_ref()),
        )
        .await;
        if !matches!(count, Ok(Some(Ok(0))))
            || self.docker_clients.has_changed().unwrap_or(true)
            || !self.docker_quiet()
        {
            return Ok(());
        }
        self.last_network_restart = Some(Instant::now());
        self.stop().await?;
        // The boot selector validates the current host policy again and only
        // starts Docker on the alternate after verifying egress there.
        self.reset_retry();
        self.begin();
        Ok(())
    }
}
