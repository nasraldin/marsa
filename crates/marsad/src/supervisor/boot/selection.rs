//! Reboot once into a verified alternate attachment before releasing the engine.

use std::time::Duration;

use marsa_vm::{Hypervisor, NetworkSpec, Vm, VmError, VmSpec};
use tokio::{
    sync::{oneshot, watch},
    time::Instant,
};

use super::{
    Activation, Failed, Issue, LiveGuest, Outcome, Reason, attach, during_guest_execution,
    ensure_stopped, graceful_stop, phase, start_engine,
};
use crate::supervisor::{
    AlternateFailure, BootStage, GuestProbe, NetworkResult,
    network::{self, Decision},
};

pub(super) async fn select_and_start(
    backend: &dyn Hypervisor,
    probe: &dyn GuestProbe,
    activation: Activation,
    cancel: &mut oneshot::Receiver<()>,
    progress: &watch::Sender<BootStage>,
) -> Result<Outcome, Failed> {
    let Activation {
        spec,
        deadline,
        stop_limit,
        selection,
    } = activation;
    let mut guest = attach(backend, probe, spec.clone(), deadline, cancel, progress).await?;
    let Some(host) = selection.host.filter(|_| spec.network == NetworkSpec::Nat) else {
        return start_engine(guest, deadline, cancel, progress).await;
    };

    progress.send_replace(BootStage::Network);
    let decision = if let Some(session) = guest.session.as_deref_mut() {
        phase(
            BootStage::Network,
            deadline,
            cancel,
            during_guest_execution(
                guest.vm.as_ref(),
                &mut guest.events,
                &mut guest.network_lost,
                async {
                    Ok(
                        network::decide(host.as_ref(), session, network::probe_deadline(deadline))
                            .await,
                    )
                },
            ),
        )
        .await
    } else {
        Err(Issue::failed(
            BootStage::Network,
            "guest session unavailable",
        ))
    };
    let (target, failure) = match decision {
        Ok(Decision::Keep(result)) => {
            guest.network.result = result;
            return start_engine(guest, deadline, cancel, progress).await;
        }
        Ok(Decision::TryUserspace { target, failure }) => (target, failure),
        Err(issue) => {
            return Err(Failed {
                vm: Some(guest.vm),
                issue,
            });
        }
    };
    retire(guest, probe, stop_limit).await?;

    let candidate = VmSpec {
        network: NetworkSpec::Userspace,
        ..spec.clone()
    };
    let now = Instant::now();
    let candidate_deadline = now + deadline.saturating_duration_since(now) / 2;
    let alternate = match attach(
        backend,
        probe,
        candidate,
        candidate_deadline,
        cancel,
        progress,
    )
    .await
    {
        Ok(mut guest) => {
            progress.send_replace(BootStage::Network);
            let checked = if guest.network_lost {
                Ok(Err(AlternateFailure::Transport))
            } else if let Some(session) = guest.session.as_deref_mut() {
                phase(
                    BootStage::Network,
                    candidate_deadline,
                    cancel,
                    during_guest_execution(
                        guest.vm.as_ref(),
                        &mut guest.events,
                        &mut guest.network_lost,
                        async {
                            Ok(network::verify_alternate(
                                host.as_ref(),
                                &target,
                                session,
                                candidate_deadline,
                            )
                            .await)
                        },
                    ),
                )
                .await
            } else {
                Ok(Err(AlternateFailure::Inconclusive))
            };
            let problem = match checked {
                Ok(Ok(())) if !guest.network_lost => {
                    guest.network.result = NetworkResult::Fallback(failure);
                    return start_engine(guest, deadline, cancel, progress).await;
                }
                Err(issue) if matches!(issue.reason, Reason::Canceled) => {
                    return Err(Failed {
                        vm: Some(guest.vm),
                        issue,
                    });
                }
                _ if guest.network_lost => AlternateFailure::Transport,
                Ok(Err(problem)) => problem,
                _ => AlternateFailure::Inconclusive,
            };
            retire(guest, probe, stop_limit).await?;
            problem
        }
        Err(failed) if matches!(failed.issue.reason, Reason::Canceled) => return Err(failed),
        Err(failed) => {
            if let Some(vm) = failed.vm {
                confirm_stopped(vm, stop_limit).await?;
            }
            AlternateFailure::Boot(failed.issue.stage)
        }
    };

    // No Docker workload was started on either attempt. Return to the native
    // attachment for local work and retain the original failure stage.
    let mut guest = attach(backend, probe, spec, deadline, cancel, progress).await?;
    guest.network.result = NetworkResult::AlternateFailed {
        native: failure,
        alternate,
    };
    start_engine(guest, deadline, cancel, progress).await
}

async fn retire(
    guest: LiveGuest,
    probe: &dyn GuestProbe,
    stop_limit: Duration,
) -> Result<(), Failed> {
    let (vm, remaining, _) = graceful_stop(guest, probe, stop_limit).await;
    confirm_stopped(vm, remaining).await
}

async fn confirm_stopped(mut vm: Box<dyn Vm>, limit: Duration) -> Result<(), Failed> {
    match ensure_stopped(vm.as_mut(), limit).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let issue = match error {
                VmError::Timeout { .. } => Issue {
                    stage: BootStage::Network,
                    reason: Reason::Timeout,
                },
                error => Issue::failed(BootStage::Network, error.to_string()),
            };
            Err(Failed {
                vm: Some(vm),
                issue,
            })
        }
    }
}
