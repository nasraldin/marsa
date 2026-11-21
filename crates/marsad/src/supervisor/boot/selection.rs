//! Reboot once into a verified alternate attachment before releasing the engine.

use std::time::Duration;

use marsa_proto::guest::v1::EgressStage;
use marsa_vm::{Hypervisor, NetworkSpec, Vm, VmSpec, VmState};
use tokio::{
    sync::{oneshot, watch},
    time::{Instant, timeout, timeout_at},
};

use super::{
    Activation, Failed, Issue, LiveGuest, Reason, attach, during_guest_execution, graceful_stop,
    phase, start_engine,
};
use crate::supervisor::{
    BootStage, GuestProbe, NetworkResult,
    network::{self, Decision},
};

pub(super) async fn select_and_start(
    backend: &dyn Hypervisor,
    probe: &dyn GuestProbe,
    activation: Activation,
    cancel: &mut oneshot::Receiver<()>,
    progress: &watch::Sender<BootStage>,
) -> Result<LiveGuest, Failed> {
    let Activation {
        spec,
        limit,
        stop_limit,
        selection,
    } = activation;
    let deadline = Instant::now() + limit;
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
            during_guest_execution(guest.vm.as_ref(), &mut guest.events, async {
                Ok(
                    network::decide(host.as_ref(), session, network::probe_deadline(deadline))
                        .await,
                )
            }),
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
    let deadline = Instant::now() + limit;
    match attach(backend, probe, candidate, deadline, cancel, progress).await {
        Ok(mut guest) => {
            progress.send_replace(BootStage::Network);
            let probe_deadline = network::probe_deadline(deadline);
            let checked = if let Some(session) = guest.session.as_deref_mut() {
                phase(
                    BootStage::Network,
                    deadline,
                    cancel,
                    during_guest_execution(guest.vm.as_ref(), &mut guest.events, async {
                        let healthy = timeout_at(probe_deadline, async {
                            session.can_probe_egress()
                                && host.verified_target().await.as_ref() == Some(&target)
                                && matches!(
                                    session.probe_egress(target.ip).await,
                                    Ok(EgressStage::Healthy)
                                )
                        })
                        .await
                        .unwrap_or(false);
                        Ok(healthy)
                    }),
                )
                .await
            } else {
                Ok(false)
            };
            match checked {
                Ok(true) => {
                    guest.network.result = NetworkResult::Fallback(failure);
                    return start_engine(guest, deadline, cancel, progress).await;
                }
                Err(issue) if matches!(issue.reason, Reason::Canceled) => {
                    return Err(Failed {
                        vm: Some(guest.vm),
                        issue,
                    });
                }
                _ => retire(guest, probe, stop_limit).await?,
            }
        }
        Err(failed) if matches!(failed.issue.reason, Reason::Canceled) => return Err(failed),
        Err(failed) => {
            if let Some(vm) = failed.vm {
                confirm_stopped(vm, stop_limit).await?;
            }
        }
    }

    // No Docker workload was started on either attempt. Return to the native
    // attachment for local work and retain the original failure stage.
    let deadline = Instant::now() + limit;
    let mut guest = attach(backend, probe, spec, deadline, cancel, progress).await?;
    guest.network.result = NetworkResult::NativeDegraded(failure);
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
    if matches!(vm.state(), VmState::Created | VmState::Stopped) {
        return Ok(());
    }
    let stopped = timeout(limit.saturating_add(Duration::from_secs(5)), vm.stop(limit)).await;
    match stopped {
        Ok(Ok(())) if vm.state() == VmState::Stopped => Ok(()),
        result => {
            let issue = match result {
                Ok(Err(error)) => Issue::failed(BootStage::Network, error.to_string()),
                Ok(Ok(())) => Issue::failed(BootStage::Network, "previous guest did not stop"),
                Err(_) => Issue {
                    stage: BootStage::Network,
                    reason: Reason::Timeout,
                },
            };
            Err(Failed {
                vm: Some(vm),
                issue,
            })
        }
    }
}
