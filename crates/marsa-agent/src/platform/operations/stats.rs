//! Sample bounded guest kernel counters and persistent disk usage.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use marsa_proto::guest::v1::VmStats;
use nix::{
    sys::{statvfs::statvfs, time::TimeValLike},
    time::{ClockId, clock_gettime},
    unistd::{SysconfVar, sysconf},
};

pub(super) const DATA_DIR: &str = "/var/lib/marsa";

pub(super) fn sample() -> io::Result<VmStats> {
    let ticks = sysconf(SysconfVar::CLK_TCK)?
        .filter(|ticks| *ticks > 0)
        .ok_or_else(invalid)? as u64;
    let uptime = clock_gettime(ClockId::CLOCK_BOOTTIME)?.num_milliseconds();
    let (memory_total_bytes, memory_used_bytes) =
        memory(&read_bounded("/proc/meminfo", 16 * 1024)?)?;
    let (data_disk_total_bytes, data_disk_used_bytes) = disk_usage()?;
    let (network_rx_bytes, network_tx_bytes) =
        network(&read_bounded("/proc/net/dev", 128 * 1024)?, |interface| {
            Path::new("/sys/class/net")
                .join(interface)
                .join("device")
                .is_symlink()
        })?;
    Ok(VmStats {
        sample_uptime_millis: uptime.try_into().map_err(|_| invalid())?,
        cpu_busy_nanos: cpu_busy(&read_bounded("/proc/stat", 128 * 1024)?, ticks)?,
        memory_total_bytes,
        memory_used_bytes,
        data_disk_total_bytes,
        data_disk_used_bytes,
        network_rx_bytes,
        network_tx_bytes,
    })
}

pub(super) fn disk_usage() -> io::Result<(u64, u64)> {
    let fs = statvfs(DATA_DIR)?;
    let block_size = fs.fragment_size();
    let total = fs.blocks().checked_mul(block_size).ok_or_else(invalid)?;
    let used = fs
        .blocks()
        .checked_sub(fs.blocks_free())
        .and_then(|blocks| blocks.checked_mul(block_size))
        .ok_or_else(invalid)?;
    Ok((total, used))
}

pub(super) fn read_bounded(path: &str, limit: usize) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(invalid());
    }
    Ok(text)
}

fn cpu_busy(stat: &str, ticks_per_second: u64) -> io::Result<u64> {
    let mut fields = stat
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("cpu "))
        .ok_or_else(invalid)?
        .split_ascii_whitespace();
    let mut ticks = 0_u128;
    for index in 0..8 {
        let field = number(fields.next())?;
        // Exclude idle and iowait. Guest ticks are included in user/nice.
        if !matches!(index, 3 | 4) {
            ticks += u128::from(field);
        }
    }
    (ticks * 1_000_000_000 / u128::from(ticks_per_second))
        .try_into()
        .map_err(|_| invalid())
}

fn memory(meminfo: &str) -> io::Result<(u64, u64)> {
    let value = |key| -> io::Result<u64> {
        let mut fields = meminfo
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .ok_or_else(invalid)?
            .split_ascii_whitespace();
        let kib = number(fields.next())?;
        if fields.next() != Some("kB") {
            return Err(invalid());
        }
        kib.checked_mul(1024).ok_or_else(invalid)
    };
    let total = value("MemTotal:")?;
    Ok((total, total.saturating_sub(value("MemAvailable:")?)))
}

fn network(netdev: &str, included: impl Fn(&str) -> bool) -> io::Result<(u64, u64)> {
    let (mut rx, mut tx) = (0_u64, 0_u64);
    for line in netdev.lines().skip(2) {
        let (interface, counts) = line.rsplit_once(':').ok_or_else(invalid)?;
        if !included(interface.trim()) {
            continue;
        }
        let mut fields = counts.split_ascii_whitespace();
        rx = rx.saturating_add(number(fields.next())?);
        tx = tx.saturating_add(number(fields.nth(7))?);
    }
    Ok((rx, tx))
}

fn number(value: Option<&str>) -> io::Result<u64> {
    value
        .and_then(|value| value.parse().ok())
        .ok_or_else(invalid)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid guest statistics")
}

#[cfg(test)]
mod tests {
    use super::{cpu_busy, memory, network};

    #[test]
    fn counters_exclude_idle_io_wait_and_virtual_interfaces() {
        assert_eq!(
            cpu_busy("cpu 2 3 10 80 25 5 4 1 5 10", 100).unwrap(),
            250_000_000
        );
        assert_eq!(
            memory("MemTotal: 1024 kB\nMemAvailable: 400 kB\n").unwrap(),
            (1_048_576, 638_976)
        );
        assert_eq!(
            network(
                "header\nheader\n lo: 800 0 0 0 0 0 0 0 900\n eth0: 120 0 0 0 0 0 0 0 99",
                |interface| interface == "eth0"
            )
            .unwrap(),
            (120, 99)
        );
    }
}
