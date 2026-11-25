//! Centralizes the Marsa per-user filesystem layout without creating files.

use std::path::{Component, Path, PathBuf};

use crate::error::PathError;
use crate::platform::HostPlatform;

/// Filename for the restricted local agent RPC socket.
pub const AGENT_SOCKET_NAME: &str = "agent.sock";

/// Host storage locations anchored to one per-user root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarsaPaths {
    root: PathBuf,
}

impl MarsaPaths {
    /// Resolves locations from the current user's home directory.
    pub fn discover(platform: &impl HostPlatform) -> Result<Self, PathError> {
        Self::from_home(platform.home_dir()?)
    }

    /// Builds the default layout inside the given absolute home directory.
    pub fn from_home(home: impl AsRef<Path>) -> Result<Self, PathError> {
        let home = home.as_ref();
        validate_directory(home)?;
        Ok(Self {
            root: home.join(".marsa"),
        })
    }

    /// Relocates the complete layout to an explicit absolute directory.
    pub fn from_root(root: impl Into<PathBuf>) -> Result<Self, PathError> {
        let root = root.into();
        validate_directory(&root)?;
        Ok(Self { root })
    }

    /// Base directory for all writable Marsa state.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Daemon-owned TOML configuration file.
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Directory for per-user runtime files.
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// Socket that serves the typed daemon API.
    pub fn daemon_socket(&self) -> PathBuf {
        self.run_dir().join("marsad.sock")
    }

    /// Socket that serves only delegated agent operations.
    pub fn agent_socket(&self) -> PathBuf {
        self.run_dir().join(AGENT_SOCKET_NAME)
    }

    /// Socket that serves the Docker Engine API.
    pub fn docker_socket(&self) -> PathBuf {
        self.run_dir().join("docker.sock")
    }

    /// Daemon process ID file.
    pub fn daemon_pid_file(&self) -> PathBuf {
        self.run_dir().join("marsad.pid")
    }

    /// Daemon single-instance lock file.
    pub fn daemon_lock_file(&self) -> PathBuf {
        self.run_dir().join("marsad.lock")
    }

    /// Directory for writable virtual machine state.
    pub fn vm_dir(&self) -> PathBuf {
        self.root.join("vm")
    }

    /// Sparse writable guest data disk image.
    pub fn data_disk(&self) -> PathBuf {
        self.vm_dir().join("data.img")
    }

    /// Captured virtual machine console output.
    pub fn console_log(&self) -> PathBuf {
        self.vm_dir().join("console.log")
    }

    /// Directory for host service logs.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Host daemon's structured log.
    pub fn daemon_log(&self) -> PathBuf {
        self.logs_dir().join("marsad.log")
    }

    /// Private directory containing locally created support archives.
    pub fn diagnostics_dir(&self) -> PathBuf {
        self.root.join("diagnostics")
    }

    /// Directory for persistent application state.
    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    /// Durable Docker inventory cache for the daemon (decision D-034).
    pub fn docker_inventory_cache(&self) -> PathBuf {
        self.state_dir().join("docker-inventory.json")
    }

    /// Stored agent permission grants.
    pub fn grants_file(&self) -> PathBuf {
        self.state_dir().join("grants.json")
    }

    /// Directory for update artifacts and staged state.
    pub fn update_dir(&self) -> PathBuf {
        self.state_dir().join("update")
    }

    /// Opt-in per-user directory for command-line tool links.
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }
}

/// Check that a path is absolute, non-root and free of traversal or NUL bytes.
pub fn validate_directory(path: &Path) -> Result<(), PathError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|part| part == Component::ParentDir)
        || path.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err(PathError::InvalidDirectory);
    }
    Ok(())
}

/// Checks the basename shape of a dated daemon JSONL log.
pub fn is_daemon_log_name(name: &str) -> bool {
    let date = name
        .strip_prefix("marsad.")
        .and_then(|name| name.strip_suffix(".jsonl"));
    let Some(date) = date else { return false };
    date.len() == 10
        && date.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}
