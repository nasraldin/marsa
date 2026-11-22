//! Decisions use the effect of a validated operation and an authenticated scope.

mod grants;
mod platform;

pub use grants::{Grant, GrantError, GrantScope, GrantStore, Grants};

use marsa_core::settings::{AgentSettings, OperateApproval};
use marsa_proto::host::v1::{AgentOperation, PermissionTier};

/// The strongest effect the daemon found while validating an operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effect {
    /// Arguments do not widen the operation's documented permission tier.
    #[default]
    Ordinary,
    /// The operation affects Mac resources, such as a new share or LAN listener.
    HostAffecting,
    /// The operation can destroy data, even when its base operation usually cannot.
    Destroy,
    /// The operation requests privileged, host-network, host-PID or Docker socket access.
    Advanced,
}

/// Container runtime settings derived from the daemon's validated operation.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContainerAccess {
    /// The request gives the container elevated Linux capabilities and devices.
    pub privileged: bool,
    /// The request joins the Linux VM's host network namespace.
    pub host_network: bool,
    /// The request joins the Linux VM's host process namespace.
    pub host_pid: bool,
    /// A resolved bind source exposes the guest Docker socket.
    pub docker_socket: bool,
}

impl ContainerAccess {
    /// Classify privileged Docker runtime features before allowing them.
    pub const fn effect(self) -> Effect {
        if self.privileged || self.host_network || self.host_pid || self.docker_socket {
            Effect::Advanced
        } else {
            Effect::Ordinary
        }
    }
}

/// Why an operation was refused before approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The operation is not defined by the typed protocol.
    UnknownOperation,
    /// The user has disabled this category of operation.
    OperateDisabled,
    /// Access needs the explicit advanced agent setting before approval can be requested.
    AdvancedDisabled,
}

/// A policy result that the daemon can convert into a structured API response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// No further approval is required.
    Allow,
    /// The user has to authorize this exact operation before it runs.
    Approve {
        /// Tier shown to the user in the approval prompt.
        tier: PermissionTier,
        /// Whether this operation may create an expiring project grant.
        can_grant_for_project: bool,
    },
    /// The operation is unavailable under the settings.
    Refuse(Refusal),
}

/// Classify every typed operation. Unknown wire values are handled before this function.
pub const fn tier(operation: AgentOperation) -> Option<PermissionTier> {
    use AgentOperation::*;

    Some(match operation {
        EngineStatus | ListContainers | InspectContainer | ContainerLogs | ContainerStats
        | ListImages | InspectImage | ListVolumes | ListNetworks | ComposeList | ComposePs
        | ComposeLogs | DiskUsage | CleanupPreview => PermissionTier::Read,
        StartContainer | StopContainer | RestartContainer | RunContainer | ExecInContainer
        | PullImage | ComposeUp | ComposeDown | ComposeRestart => PermissionTier::Operate,
        RemoveContainer | RemoveImage | RemoveNetwork | RemoveVolume | ComposeDownVolumes
        | Prune => PermissionTier::Destroy,
        AddShare => PermissionTier::HostAffecting,
        Unspecified => return None,
    })
}

/// Evaluate a request without a project grant after daemon argument validation.
///
/// A successful decision does not authorize a bind source. Host sharing policy must
/// separately approve every host path.
pub const fn decide(
    operation: AgentOperation,
    effect: Effect,
    settings: &AgentSettings,
) -> Decision {
    let Some(base_tier) = tier(operation) else {
        return Decision::Refuse(Refusal::UnknownOperation);
    };
    if matches!(effect, Effect::Advanced) && !settings.advanced_access {
        return Decision::Refuse(Refusal::AdvancedDisabled);
    }
    if matches!(base_tier, PermissionTier::Operate)
        && matches!(settings.operate_approval, OperateApproval::Deny)
    {
        return Decision::Refuse(Refusal::OperateDisabled);
    }
    let effective_tier = match effect {
        Effect::Advanced | Effect::HostAffecting => PermissionTier::HostAffecting,
        Effect::Destroy => PermissionTier::Destroy,
        Effect::Ordinary => base_tier,
    };
    let grantable = matches!(effective_tier, PermissionTier::Operate)
        && matches!(settings.operate_approval, OperateApproval::PerProject);
    if matches!(effective_tier, PermissionTier::Read) {
        return Decision::Allow;
    }
    Decision::Approve {
        tier: effective_tier,
        can_grant_for_project: grantable,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
