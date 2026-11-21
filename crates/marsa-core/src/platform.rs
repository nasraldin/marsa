//! Platform services stay behind a trait so host path resolution is testable.

use std::path::PathBuf;

use crate::error::PathError;

/// Host services needed by shared Marsa code.
pub trait HostPlatform {
    /// Returns the current user's home directory.
    fn home_dir(&self) -> Result<PathBuf, PathError>;
}

/// The macOS host implementation of shared platform services.
#[cfg(target_os = "macos")]
#[derive(Debug, Default, Clone, Copy)]
pub struct MacOs;

#[cfg(target_os = "macos")]
impl HostPlatform for MacOs {
    fn home_dir(&self) -> Result<PathBuf, PathError> {
        std::env::home_dir().ok_or(PathError::HomeUnavailable)
    }
}
