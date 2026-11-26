//! Confirm and apply a single staged daemon proxy with an exclusive lifecycle permit.

use std::time::Duration;

use marsa_core::settings::NetworkMode;
use marsa_proto::host::v1::{
    ActivateProxyRequest, ProxyActivationFailureStage as Stage, ProxyActivationImpact,
    ProxyActivationOutcome as Outcome, ProxyActivationPreview, ProxyActivationResult,
};
use tokio::time::Instant;
use tonic::Status;

use crate::supervisor::{State, Supervisor, SupervisorError};

use super::{
    TypedApi,
    proxy_policy::{BootProxy, RuntimeProxy},
};

const APPROVAL_SECONDS: u32 = 90;

pub(super) struct Approval {
    token: [u8; 32],
    settings_revision: u64,
    lifecycle_revision: u64,
    interrupts_engine: bool,
    candidate: BootProxy,
    expires: Instant,
}

impl TypedApi {
    fn proxy_runtime(&self) -> Result<(&RuntimeProxy, &Supervisor), Status> {
        self.proxy
            .as_deref()
            .zip(self.supervisor.as_ref())
            .ok_or_else(|| Status::failed_precondition("packaged proxy activation is unavailable"))
    }

    pub(super) async fn preview_proxy(
        &self,
        expected: u64,
    ) -> Result<ProxyActivationPreview, Status> {
        let _edit = self.inner.edit.lock().await;
        let (runtime, supervisor) = self.proxy_runtime()?;
        let candidate = self.draft(runtime, expected).await?;
        let engine = supervisor.snapshot();
        let interrupts_engine = match engine.state {
            State::Ready | State::Idle | State::Paused => true,
            State::Stopped => false,
            _ => {
                return Err(Status::failed_precondition(
                    "guest lifecycle is transitioning",
                ));
            }
        };
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|_| Status::internal("could not create proxy approval"))?;
        let fixed = candidate
            .guest_configuration()
            .and_then(|config| config.proxy)
            .unwrap_or_default();
        let approval = Approval {
            token,
            candidate,
            settings_revision: expected,
            lifecycle_revision: engine.lifecycle_revision,
            interrupts_engine,
            expires: Instant::now() + Duration::from_secs(APPROVAL_SECONDS.into()),
        };
        let mut pending = self
            .inner
            .proxy_approval
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *pending = Some(approval);
        Ok(ProxyActivationPreview {
            http_proxy: fixed.http_url,
            https_proxy: fixed.https_url,
            no_proxy: fixed.no_proxy,
            approval_token: token.to_vec(),
            settings_revision: expected,
            interrupts_running_engine: interrupts_engine,
            approval_valid_seconds: APPROVAL_SECONDS,
            potential_interruptions: if interrupts_engine {
                [
                    ProxyActivationImpact::Containers,
                    ProxyActivationImpact::Builds,
                    ProxyActivationImpact::AttachedStreams,
                    ProxyActivationImpact::SessionShares,
                ]
                .into_iter()
                .map(i32::from)
                .collect()
            } else {
                Vec::new()
            },
        })
    }

    async fn draft(&self, runtime: &RuntimeProxy, expected: u64) -> Result<BootProxy, Status> {
        let (settings, snapshot) = self.inner.events.settings();
        if snapshot.revision != expected {
            return Err(Status::aborted("settings revision changed"));
        }
        if settings.network.mode != NetworkMode::Userspace {
            return Err(Status::failed_precondition(
                "daemon proxy activation requires user-space networking",
            ));
        }
        let candidate =
            super::system_proxy::resolve(&settings.network.proxy, self.proxy_discovery.as_ref())
                .await?;
        if candidate == runtime.approved() {
            return Err(Status::failed_precondition(
                "daemon proxy settings have not changed",
            ));
        }
        Ok(candidate)
    }

    fn consume_proxy_approval(&self, request: &ActivateProxyRequest) -> Result<Approval, Status> {
        if !request.approved || request.approval_token.len() != 32 {
            return Err(Status::failed_precondition(
                "explicit proxy approval is required",
            ));
        }
        let mut pending = self
            .inner
            .proxy_approval
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(approval) = pending.as_ref()
            && approval.token == request.approval_token.as_slice()
            && approval.settings_revision == request.expected_settings_revision
            && Instant::now() < approval.expires
        {
            return pending
                .take()
                .ok_or_else(|| Status::aborted("proxy approval is unavailable"));
        }
        Err(Status::aborted("proxy approval is stale or unavailable"))
    }

    pub(super) async fn apply_proxy(
        &self,
        request: ActivateProxyRequest,
    ) -> Result<ProxyActivationResult, Status> {
        // The request cache owns this worker after confirmation, including when
        // the client disconnects mid-restart. Settings cannot race the operation.
        let _edit = self.inner.edit.lock().await;
        let (runtime, supervisor) = self.proxy_runtime()?;
        let approval = self.consume_proxy_approval(&request)?;
        let candidate = self.draft(runtime, approval.settings_revision).await?;
        if candidate != approval.candidate {
            return Err(Status::aborted(
                "system proxy changed; prepare a new approval | تغير وكيل النظام. اطلب موافقة جديدة",
            ));
        }
        let previous = self.inner.events.startup_settings();
        let effective = candidate.effective_on_userspace(previous.clone());

        let paths = self.inner.paths.clone();
        let uid = self.inner.uid;
        let policy = candidate.clone();
        let staged = tokio::task::spawn_blocking(move || policy.stage(&paths, uid))
            .await
            .map_err(|_| Status::internal("proxy policy staging failed"))?
            .map_err(|_| Status::internal("proxy policy staging failed"))?;

        let permit = supervisor
            .reserve(Some(approval.lifecycle_revision))
            .await
            .map_err(|error| match error {
                SupervisorError::StaleRevision { .. } => Status::aborted("guest lifecycle changed"),
                SupervisorError::Busy => Status::unavailable("guest lifecycle admission is busy"),
                _ => Status::unavailable("guest supervisor is unavailable"),
            })?;
        let mut closing = self.inner.closing.subscribe();
        if *closing.borrow() {
            return Err(Status::unavailable("daemon is shutting down"));
        }
        if permit.stop().await.is_err() {
            return Ok(result(Outcome::EngineRecoveryRequired, Stage::Stop));
        }
        let attempt = {
            let _policy = runtime.override_for_permit(effective.clone(), candidate.clone());
            tokio::select! {
                biased;
                _ = closing.wait_for(|closing| *closing) => Err(Stage::Shutdown),
                attempt = permit.start_candidate() => attempt.map_err(|_| Stage::Start),
            }
        };

        let failure = match attempt {
            Ok(()) if !*closing.borrow() => {
                match tokio::task::spawn_blocking(move || staged.commit()).await {
                    Ok(Ok(durable)) => {
                        runtime.replace_approved(candidate);
                        self.inner.events.system_proxy_pending(false);
                        self.inner.events.runtime_applied(&effective);
                        // If the actor has already shut down, the freshly committed
                        // policy still owns the next boot. Closing drops the guest.
                        let _ = permit.keep_candidate().await;
                        return Ok(result(
                            if durable {
                                Outcome::Applied
                            } else {
                                Outcome::AppliedDurabilityUncertain
                            },
                            Stage::Unspecified,
                        ));
                    }
                    _ => Stage::Persist,
                }
            }
            Ok(()) => Stage::Shutdown,
            Err(stage) => stage,
        };

        if permit.stop().await.is_err() {
            return Ok(result(Outcome::EngineRecoveryRequired, failure));
        }
        if !approval.interrupts_engine || *closing.borrow() {
            return Ok(result(Outcome::PriorPolicyKept, failure));
        }
        let _policy = runtime.override_for_permit(previous, runtime.approved());
        let restored = tokio::select! {
            biased;
            _ = closing.wait_for(|closing| *closing) => false,
            outcome = permit.start() => outcome.is_ok(),
        };
        if restored {
            Ok(result(Outcome::PriorEngineRestored, failure))
        } else {
            Ok(result(Outcome::EngineRecoveryRequired, failure))
        }
    }
}

fn result(outcome: Outcome, failure: Stage) -> ProxyActivationResult {
    ProxyActivationResult {
        outcome: outcome.into(),
        failure_stage: failure.into(),
    }
}

#[cfg(test)]
mod tests;
