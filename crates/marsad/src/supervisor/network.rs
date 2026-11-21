//! Qualify network fallback while the guest engine is still gated.

use std::{sync::Arc, time::Duration};

use marsa_proto::guest::v1::EgressStage;
use marsa_vm::{NetworkSpec, async_trait};
use tokio::time::{Instant, timeout_at};

use super::{
    BootStage, GuestSession,
    host_egress::{self, Target},
};

#[derive(Clone)]
pub(super) struct Selection {
    pub(super) host: Option<Arc<dyn HostCheck>>,
}

impl Selection {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            host: enabled.then(|| Arc::new(SystemHost) as Arc<dyn HostCheck>),
        }
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
    /// Native NAT failed at this stage and user-space egress was verified.
    Fallback(EgressStage),
    /// Native NAT reported this stage and no working alternate path was confirmed.
    NativeDegraded(EgressStage),
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
    let Some((stage, retry)) = timeout_at(deadline, compare(host, session))
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

async fn compare(
    host: &dyn HostCheck,
    session: &mut dyn GuestSession,
) -> Option<(EgressStage, Option<Target>)> {
    let target = host.verified_target().await?;
    let first = session
        .probe_egress(target.ip)
        .await
        .inspect_err(|error| {
            tracing::debug!(kind = ?error.kind(), "automatic boot guest comparison unavailable");
        })
        .ok()?;
    tracing::debug!(stage = ?first, "automatic boot native egress check");
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
    let second = session.probe_egress(target.ip).await.ok()?;
    Some((second, (first == second).then_some(target)))
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
        let stage = session
            .probe_egress(target.ip)
            .await
            .map_err(|_| AlternateFailure::Inconclusive)?;
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
