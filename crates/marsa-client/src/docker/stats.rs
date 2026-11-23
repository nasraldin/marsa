//! Bounded counters for desktop consumption of Docker's container stats stream.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::null_to_default;

/// One sample from the Docker stats stream.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatsSample {
    /// Container memory usage and limit.
    #[serde(default)]
    pub memory_stats: MemoryStats,
    /// Aggregate CPU counters.
    #[serde(default)]
    pub cpu_stats: CpuStats,
    /// Previous CPU counters included in the same sample by the engine.
    #[serde(default)]
    pub precpu_stats: CpuStats,
    /// Per-interface transfer counters.
    #[serde(default, deserialize_with = "null_to_default")]
    pub networks: BTreeMap<String, NetworkCounters>,
    /// Per-device Linux I/O counters.
    #[serde(default)]
    pub blkio_stats: BlockIoStats,
    /// Process counts when the engine supplies them.
    #[serde(default)]
    pub pids_stats: ProcessStats,
    /// Timestamp supplied by the engine.
    pub read: Option<String>,
}

impl StatsSample {
    /// CPU usage as a percentage of one logical CPU, when both samples are available.
    pub fn cpu_percent(&self) -> Option<f64> {
        let current = self.cpu_stats.system_cpu_usage?;
        let previous = self.precpu_stats.system_cpu_usage?;
        let system = current.checked_sub(previous).filter(|delta| *delta > 0)?;
        let usage = self
            .cpu_stats
            .cpu_usage
            .total_usage?
            .checked_sub(self.precpu_stats.cpu_usage.total_usage?)?;
        let cpus = self
            .cpu_stats
            .online_cpus
            .filter(|cpus| *cpus > 0)
            .or_else(|| u32::try_from(self.cpu_stats.cpu_usage.percpu_usage.len()).ok())
            .filter(|cpus| *cpus > 0)?;
        Some(100.0 * usage as f64 / system as f64 * f64::from(cpus))
    }

    /// Cumulative received and transmitted bytes across interfaces.
    pub fn network_bytes(&self) -> (u64, u64) {
        self.networks
            .values()
            .fold((0u64, 0u64), |(rx, tx), counters| {
                (
                    rx.saturating_add(counters.rx_bytes),
                    tx.saturating_add(counters.tx_bytes),
                )
            })
    }

    /// Cumulative read and written bytes across Linux block devices.
    pub fn block_bytes(&self) -> (u64, u64) {
        self.blkio_stats.io_service_bytes_recursive.iter().fold(
            (0u64, 0u64),
            |(read, write), entry| {
                if entry.op.eq_ignore_ascii_case("read") {
                    (read.saturating_add(entry.value), write)
                } else if entry.op.eq_ignore_ascii_case("write") {
                    (read, write.saturating_add(entry.value))
                } else {
                    (read, write)
                }
            },
        )
    }
}

/// Cumulative CPU counters from Docker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CpuStats {
    /// CPU time consumed by the container.
    #[serde(default)]
    pub cpu_usage: CpuUsage,
    /// CPU time consumed by the host since boot.
    pub system_cpu_usage: Option<u64>,
    /// Number of online logical CPUs when reported.
    pub online_cpus: Option<u32>,
}

/// CPU usage reported for the container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CpuUsage {
    /// Total container CPU time in nanoseconds, where reported.
    pub total_usage: Option<u64>,
    /// Per-CPU time on kernels that report these counters.
    #[serde(default, deserialize_with = "null_to_default")]
    pub percpu_usage: Vec<u64>,
}

/// Container memory counters from Docker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MemoryStats {
    /// Current total usage in bytes.
    pub usage: Option<u64>,
    /// Limit in bytes.
    pub limit: Option<u64>,
    /// Linux cgroup counters including inactive file cache.
    #[serde(default, deserialize_with = "null_to_default")]
    pub stats: BTreeMap<String, u64>,
}

impl MemoryStats {
    /// Memory usage with the Linux inactive file cache subtracted.
    pub fn used_without_cache(&self) -> Option<u64> {
        let usage = self.usage?;
        let cache = self
            .stats
            .get("total_inactive_file")
            .or_else(|| self.stats.get("inactive_file"))
            .copied()
            .filter(|cache| *cache < usage)
            .unwrap_or_default();
        Some(usage - cache)
    }
}

/// Cumulative device I/O for a container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct BlockIoStats {
    /// Transfer counts in bytes grouped by device and operation.
    #[serde(default, deserialize_with = "null_to_default")]
    pub io_service_bytes_recursive: Vec<BlockIoCounter>,
}

/// One engine-reported block device transfer counter.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BlockIoCounter {
    /// Engine label for the operation.
    pub op: String,
    /// Cumulative transfer count in bytes.
    pub value: u64,
}

/// Cumulative transfer counters for one interface.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NetworkCounters {
    /// Cumulative received bytes.
    pub rx_bytes: u64,
    /// Cumulative transmitted bytes.
    pub tx_bytes: u64,
}

/// Container process counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProcessStats {
    /// Current process count when reported.
    pub current: Option<u64>,
    /// Process limit when reported.
    pub limit: Option<u64>,
}
