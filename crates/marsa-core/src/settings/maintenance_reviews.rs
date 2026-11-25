//! Owner opt-ins for recurring read-only cleanup reviews.

use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::paths::validate_directory;

/// Interval between scheduled review attempts in an open desktop workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCadence {
    /// Review every hour.
    Hourly,
    /// Review every 24 hours.
    Daily,
    /// Review every seven days.
    Weekly,
}

impl ReviewCadence {
    /// The fixed interval starts when the desktop reads or changes the schedule.
    pub const fn interval(self) -> Duration {
        Duration::from_secs(match self {
            Self::Hourly => 3_600,
            Self::Daily => 86_400,
            Self::Weekly => 604_800,
        })
    }
}

/// Persisted schedule. The missing map entry is the opt-out.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceReview {
    /// Repeat period of the read-only review.
    pub cadence: ReviewCadence,
    /// Engine selected when configuring the schedule.
    pub target: ReviewTarget,
}

/// Target information required to resolve the exact engine at review time.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewTarget {
    /// Owner's built-in Marsa engine.
    Builtin,
    /// A saved owner-authenticated local Docker connection.
    ManagedUnix {
        /// Canonical socket path captured when configuring the schedule.
        socket: PathBuf,
        /// Engine ID observed at that path.
        engine_id: String,
    },
}

impl MaintenanceReview {
    pub(super) fn valid_for(&self, key: &str) -> bool {
        match &self.target {
            ReviewTarget::Builtin => key == "marsa",
            ReviewTarget::ManagedUnix { socket, engine_id } => {
                key.strip_prefix("managed:").is_some_and(|id| {
                    id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                }) && validate_directory(socket).is_ok()
                    && socket.to_str().is_some_and(|path| valid_text(path, 4096))
                    && valid_text(engine_id, 256)
            }
        }
    }
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
