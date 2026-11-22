//! Virtual machine and data disk resource preferences.

use serde::{Deserialize, Serialize};

use crate::error::SettingsError;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const MIN_VM_MEMORY: u64 = 512 * MIB;
const DEFAULT_VM_MEMORY_CAP: u64 = 8 * GIB;

/// Host hypervisor selected for the Linux virtual machine.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmBackend {
    /// Apple's Virtualization.framework backend.
    #[default]
    Vz,
    /// The alternative libkrun backend.
    Krun,
}

/// Resource limits and idle behavior for the virtual machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VmSettings {
    /// Number of guest virtual CPUs.
    pub cpus: u16,
    /// Explicit guest memory in bytes. Omit it to size memory at boot.
    pub memory: Option<u64>,
    /// Hypervisor to request when starting the virtual machine.
    pub backend: VmBackend,
    /// Stop the virtual machine when no containers are running.
    pub idle_stop: bool,
    /// Seconds to wait before stopping an idle virtual machine.
    pub idle_timeout: u32,
}

impl Default for VmSettings {
    fn default() -> Self {
        Self {
            cpus: 2,
            memory: None,
            backend: VmBackend::default(),
            idle_stop: true,
            idle_timeout: 300,
        }
    }
}

impl VmSettings {
    /// Returns the requested guest memory, using half of host RAM capped at 8 GiB by default.
    ///
    /// The hypervisor still checks hardware limits when the virtual machine starts.
    pub fn memory_bytes(&self, host_memory_bytes: u64) -> u64 {
        self.memory
            .unwrap_or_else(|| (host_memory_bytes / 2).min(DEFAULT_VM_MEMORY_CAP))
    }

    pub(super) fn validate(&self) -> Result<(), SettingsError> {
        if self.cpus == 0 {
            return Err(SettingsError::invalid("vm.cpus", "must be at least one"));
        }
        if self
            .memory
            .is_some_and(|bytes| bytes < MIN_VM_MEMORY || i64::try_from(bytes).is_err())
        {
            return Err(SettingsError::invalid(
                "vm.memory",
                "must be at least 512 MiB and fit in a signed 64-bit byte count",
            ));
        }
        if self.idle_timeout == 0 {
            return Err(SettingsError::invalid(
                "vm.idle_timeout",
                "must be at least one second",
            ));
        }
        Ok(())
    }
}

/// Preferences for the guest's sparse data disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiskSettings {
    /// Maximum size of the sparse data disk, in bytes.
    pub data_size_limit: u64,
    /// Automatically reclaim unused blocks where the backend supports it.
    pub auto_reclaim: bool,
    /// Exclude the raw image from Time Machine backups on macOS.
    pub exclude_from_backup: bool,
}

impl Default for DiskSettings {
    fn default() -> Self {
        Self {
            data_size_limit: 64 * GIB,
            auto_reclaim: true,
            exclude_from_backup: true,
        }
    }
}

impl DiskSettings {
    pub(super) fn validate(&self) -> Result<(), SettingsError> {
        if self.data_size_limit < GIB || i64::try_from(self.data_size_limit).is_err() {
            return Err(SettingsError::invalid(
                "disk.data_size_limit",
                "must be at least 1 GiB and fit in a signed 64-bit byte count",
            ));
        }
        Ok(())
    }
}
