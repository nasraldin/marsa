//! Carry out privileged guest commands using bounded local inputs.

pub(super) mod ports;
mod shares;
mod stats;

use std::{io, process::Stdio};

use marsa_proto::guest::v1::{
    TrimResult, VmStats, guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    task::spawn_blocking,
    time::{Duration, timeout},
};

pub(super) async fn command(body: HostBody) -> io::Result<Option<GuestBody>> {
    match body {
        HostBody::Trim(_) => return trim().await.map(GuestBody::TrimResult).map(Some),
        HostBody::MountShare(request) => spawn_blocking(move || shares::attach(request)).await??,
        HostBody::UnmountShare(request) => {
            spawn_blocking(move || shares::detach(request)).await??
        }
        HostBody::Shutdown(_) => spawn_blocking(verify_init).await??,
        _ => return Ok(None),
    }
    // The session sends the result before signaling PID 1.
    Ok(Some(crate::control::success("Guest operation completed")))
}

pub(super) fn shutdown() -> io::Result<()> {
    kill(Pid::from_raw(1), Signal::SIGTERM)?;
    Ok(())
}

pub(super) async fn stats() -> io::Result<VmStats> {
    spawn_blocking(stats::sample).await?
}

fn verify_init() -> io::Result<()> {
    if stats::read_bounded("/proc/1/comm", 64)?.trim_end() != "marsa-init" {
        return Err(io::Error::other("shutdown requires marsa-init as PID 1"));
    }
    Ok(())
}

async fn trim() -> io::Result<TrimResult> {
    let mut child = Command::new("/sbin/fstrim")
        .args(["--verbose", stats::DATA_DIR])
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut output = String::new();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("fstrim stdout is unavailable"))?;
    timeout(Duration::from_secs(30), async {
        stdout.take(1025).read_to_string(&mut output).await?;
        if output.len() > 1024 || !child.wait().await?.success() {
            return Err(io::Error::other(
                "fstrim failed or returned oversized output",
            ));
        }
        Ok(())
    })
    .await??;
    let trimmed = trim_bytes(&output)?;
    let (_, used) = spawn_blocking(stats::disk_usage).await??;
    Ok(TrimResult {
        bytes_trimmed: trimmed,
        data_disk_used_bytes: used,
    })
}

fn trim_bytes(output: &str) -> io::Result<u64> {
    // util-linux emits `/var/lib/marsa: <size> (N bytes) trimmed` in the C locale.
    output
        .trim_end()
        .strip_prefix("/var/lib/marsa: ")
        .and_then(|rest| rest.strip_suffix(" bytes) trimmed"))
        .and_then(|rest| rest.rsplit_once(" ("))
        .map(|(_, bytes)| bytes)
        .and_then(|bytes| bytes.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid fstrim output"))
}

#[cfg(test)]
mod tests {
    use super::trim_bytes;

    #[test]
    fn decodes_c_locale_byte_counts_only() {
        assert_eq!(
            trim_bytes("/var/lib/marsa: 53.4 MiB (56041472 bytes) trimmed\n").unwrap(),
            56_041_472
        );
        assert!(trim_bytes("/var/lib/marsa: 12 MiB trimmed\n").is_err());
    }
}
