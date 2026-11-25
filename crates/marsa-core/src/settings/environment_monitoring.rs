//! Desktop monitoring preferences scoped to a stable environment identity.

use serde::{Deserialize, Serialize};

/// Activity history requested from the selected Docker engine.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityRetention {
    /// Request up to one hour of retained events.
    OneHour,
    /// Request up to six hours of retained events.
    SixHours,
    /// Request up to twenty-four hours of retained events.
    #[default]
    OneDay,
}

impl ActivityRetention {
    /// Docker event lookback in hours.
    pub const fn hours(self) -> u32 {
        match self {
            Self::OneHour => 1,
            Self::SixHours => 6,
            Self::OneDay => 24,
        }
    }
}

/// Local preferences for observing and reviewing one Docker environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnvironmentMonitoring {
    /// Load recent Docker activity for this environment.
    pub collect_activity: bool,
    /// Maximum age requested for retained activity.
    pub activity_retention: ActivityRetention,
    /// Check whether images used by containers have newer registry digests.
    pub check_image_updates: bool,
    /// List unused volumes as candidates when reviewing storage.
    pub show_volume_candidates: bool,
    /// Show a local notification after a container becomes unhealthy.
    pub notify_unhealthy: bool,
}

impl Default for EnvironmentMonitoring {
    fn default() -> Self {
        Self {
            collect_activity: true,
            activity_retention: ActivityRetention::default(),
            check_image_updates: true,
            show_volume_candidates: true,
            notify_unhealthy: false,
        }
    }
}
