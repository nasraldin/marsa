//! Qualify network fallback while the guest engine is still gated.

use std::{io, net::Ipv4Addr, sync::Arc, time::Duration};

use marsa_core::settings::NetworkMode;
use marsa_proto::guest::v1::EgressStage;
use marsa_vm::{NetworkSpec, async_trait};
use tokio::time::{Instant, timeout_at};

use super::{
    BootStage, GuestSession,
    host_egress::{self, Target},
};

#[derive(Clone)]
pub(crate) struct Selection {
    pub(super) host: Option<Arc<dyn HostCheck>>,
    pub(super) configured_mode: Option<NetworkMode>,
}

impl Selection {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            host: enabled.then(|| Arc::new(SystemHost) as Arc<dyn HostCheck>),
            configured_mode: None,
        }
    }
    pub(crate) fn with_configured_mode(mut self, mode: NetworkMode) -> Self {
        self.configured_mode = Some(mode);
        self
    }
}

#[async_trait]
pub(super) trait HostCheck: Send + Sync {
    async fn verified_target(&self) -> Option<Target>;
}

struct SystemHost;

#[async_trait]
impl HostCheck for SystemHost {
    async fn verified_target(&self) -> Option<Target> {
        host_egress::verified_target().await
    }
}

/// The active attachment and the result of boot-time network selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootNetwork {
    /// Attachment used by the running guest.
    pub active: NetworkSpec,
    /// Whether use of the alternate attachment was verified.
    pub result: NetworkResult,
}

/// Result of automatic egress checks, independent of Docker readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkResult {
    /// The configured attachment was used without an automatic probe.
    Unchecked,
    /// The host or guest policy prevented a comparable probe.
    Unavailable,
    /// Native NAT reached the verified IPv4 HTTPS destination.
    NativeHealthy,
    /// A pinned attachment completed a live DNS, TCP, TLS and HTTP registry check.
    PinnedHealthy,
    /// Native NAT failed at this stage and user-space egress was verified.
    Fallback(EgressStage),
    /// Native NAT reported this stage and no working alternate path was confirmed.
    NativeDegraded(EgressStage),
    /// Repeated native failure confirmed on a running VM. Cold selection is pending.
    RestartForUserspace(EgressStage),
    /// The running user-space guest reported this failure without switching attachments.
    UserspaceDegraded(EgressStage),
    /// The original native failure and the independent alternate-path failure.
    AlternateFailed {
        /// Stage reported by the native guest.
        native: EgressStage,
        /// Outcome of the alternate attachment or probe.
        alternate: AlternateFailure,
    },
    /// The configured attachment lost its transport during guest startup.
    TransportUnavailable,
}

impl NetworkResult {
    pub(crate) fn restart_pending(self, desired: NetworkMode) -> bool {
        self == Self::TransportUnavailable
            || (desired == NetworkMode::Auto && matches!(self, Self::RestartForUserspace(_)))
    }
}

/// Where the alternate attachment failed during automatic boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlternateFailure {
    /// Creating or starting the alternate guest failed.
    Boot(BootStage),
    /// The alternate guest reported a failed probe stage.
    Egress(EgressStage),
    /// The network helper stopped during activation.
    Transport,
    /// The host and alternate guest could not be compared.
    Inconclusive,
}

pub(super) enum Decision {
    Keep(NetworkResult),
    TryUserspace {
        target: Target,
        failure: EgressStage,
    },
}

pub(super) async fn decide(
    host: &dyn HostCheck,
    session: &mut dyn GuestSession,
    deadline: Instant,
) -> Decision {
    if !session.can_probe_egress() {
        return Decision::Keep(NetworkResult::Unavailable);
    }
    let Some((stage, retry)) = timeout_at(deadline, compare(host, Probe::Boot(session)))
        .await
        .ok()
        .flatten()
    else {
        return Decision::Keep(NetworkResult::Unavailable);
    };
    match (stage, retry) {
        (EgressStage::Healthy, _) => Decision::Keep(NetworkResult::NativeHealthy),
        (_, Some(target)) => Decision::TryUserspace {
            target,
            failure: stage,
        },
        _ => Decision::Keep(NetworkResult::NativeDegraded(stage)),
    }
}

enum Probe<'a> {
    Boot(&'a mut dyn GuestSession),
    Live(&'a mut dyn super::LiveEgressProbe),
}

impl Probe<'_> {
    async fn run(&mut self, ip: Ipv4Addr) -> io::Result<EgressStage> {
        match self {
            Self::Boot(session) => session.probe_egress(ip).await,
            Self::Live(session) => session.probe(ip).await,
        }
    }
}

async fn compare(
    host: &dyn HostCheck,
    mut probe: Probe<'_>,
) -> Option<(EgressStage, Option<Target>)> {
    let target = host.verified_target().await?;
    let first = probe
        .run(target.ip)
        .await
        .inspect_err(|error| {
            tracing::debug!(kind = ?error.kind(), "registry egress comparison unavailable");
        })
        .ok()?;
    tracing::debug!(stage = ?first, "registry egress check");
    if !matches!(
        first,
        EgressStage::Dns | EgressStage::Route | EgressStage::Tcp
    ) {
        return Some((first, None));
    }
    // Recheck policy and reachability between guest attempts. The host check
    // returns only a stable, direct IPv4 target with verified TLS.
    if host.verified_target().await.as_ref() != Some(&target) {
        return Some((first, None));
    }
    let second = probe.run(target.ip).await.ok()?;
    Some((second, (first == second).then_some(target)))
}

pub(crate) struct LiveCheck {
    pub(super) host: Arc<dyn HostCheck>,
    pub(super) probe: Box<dyn super::LiveEgressProbe>,
    pub(super) network: BootNetwork,
    pub(super) automatic: bool,
    pub(super) generation: u64,
}

impl LiveCheck {
    pub(crate) fn observe_only(&mut self) {
        self.automatic = false;
    }

    pub(crate) async fn run(&mut self) -> NetworkResult {
        if !self.automatic {
            let stage = tokio::time::timeout(Duration::from_secs(18), async {
                let target = self.host.verified_target().await?;
                self.probe.probe(target.ip).await.ok()
            })
            .await
            .ok()
            .flatten();
            return match (self.network.active, stage) {
                (_, None) => NetworkResult::Unavailable,
                (_, Some(EgressStage::Healthy)) => NetworkResult::PinnedHealthy,
                (NetworkSpec::Nat, Some(stage)) => NetworkResult::NativeDegraded(stage),
                (NetworkSpec::Userspace, Some(stage)) => NetworkResult::UserspaceDegraded(stage),
            };
        }
        let observed = tokio::time::timeout(
            Duration::from_secs(28),
            compare(self.host.as_ref(), Probe::Live(self.probe.as_mut())),
        )
        .await
        .ok()
        .flatten();
        match (self.network.active, observed) {
            (_, None) => NetworkResult::Unavailable,
            (NetworkSpec::Nat, Some((EgressStage::Healthy, _))) => NetworkResult::NativeHealthy,
            (NetworkSpec::Nat, Some((stage, Some(_)))) => NetworkResult::RestartForUserspace(stage),
            (NetworkSpec::Nat, Some((stage, None))) => NetworkResult::NativeDegraded(stage),
            (NetworkSpec::Userspace, Some((EgressStage::Healthy, _))) => {
                match self.network.result {
                    previous @ NetworkResult::Fallback(_) => previous,
                    _ => NetworkResult::Unchecked,
                }
            }
            (NetworkSpec::Userspace, Some((stage, _))) => NetworkResult::UserspaceDegraded(stage),
        }
    }
}

pub(super) async fn verify_alternate(
    host: &dyn HostCheck,
    target: &Target,
    session: &mut dyn GuestSession,
    deadline: Instant,
) -> Result<(), AlternateFailure> {
    timeout_at(deadline, async {
        if !session.can_probe_egress() || host.verified_target().await.as_ref() != Some(target) {
            return Err(AlternateFailure::Inconclusive);
        }
        let probe =
            |stage: std::io::Result<EgressStage>| stage.map_err(|_| AlternateFailure::Inconclusive);
        let mut stage = probe(session.probe_egress(target.ip).await)?;
        // A firewall may defer the first outbound connection while it asks the
        // owner. Give the same attachment one bounded retry through that policy.
        if stage == EgressStage::Tcp {
            if host.verified_target().await.as_ref() != Some(target) {
                return Err(AlternateFailure::Inconclusive);
            }
            stage = probe(session.probe_egress(target.ip).await)?;
        }
        if stage != EgressStage::Healthy {
            return Err(AlternateFailure::Egress(stage));
        }
        if host.verified_target().await.as_ref() != Some(target) {
            return Err(AlternateFailure::Inconclusive);
        }
        Ok(())
    })
    .await
    .unwrap_or(Err(AlternateFailure::Inconclusive))
}

pub(super) fn probe_deadline(boot_deadline: Instant) -> Instant {
    // Reserve time for Docker even if the network is offline. Individual host
    // and guest operations also have transport time limits.
    let now = Instant::now();
    let budget = boot_deadline.saturating_duration_since(now) / 2;
    now + budget.min(Duration::from_secs(28))
}
