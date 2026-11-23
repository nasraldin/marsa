//! Owner preferences for the identity shown for local and discovered environments.

use serde::{Deserialize, Serialize};

/// An icon available in every Marsa installation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentIcon {
    /// General Docker server.
    #[default]
    Server,
    /// Local machine or launch environment.
    Rocket,
    /// Remote or shared environment.
    Globe,
    /// Container workspace.
    Box,
}

/// Display preferences that do not modify a discovered Docker context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentAppearance {
    /// User-chosen display name.
    pub name: String,
    /// User-chosen icon.
    pub icon: EnvironmentIcon,
}
