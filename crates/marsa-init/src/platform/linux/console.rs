//! Replay the early kernel messages lost by the pinned virtio console driver.

use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

// Greater than the pinned kernel's 128 KiB ring. Never consume or clear it.
const SNAPSHOT_BYTES: &str = "262144";

pub(super) fn replay_early_kernel_log() -> io::Result<()> {
    let output = Command::new("/bin/busybox")
        .args(["dmesg", "-s", SNAPSHOT_BYTES])
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "busybox dmesg returned {}",
            output.status
        )));
    }
    let snapshot = String::from_utf8_lossy(&output.stdout);
    if let Some(early) = before_hvc(&snapshot) {
        let mut console = io::stderr().lock();
        writeln!(console, "marsa-init: replaying early kernel boot log")?;
        console.write_all(early.as_bytes())?;
        writeln!(console, "marsa-init: end of early kernel boot log")?;
    }
    Ok(())
}

fn before_hvc(snapshot: &str) -> Option<&str> {
    let boundary = "printk: legacy console [hvc0] enabled";
    let mut length = 0;
    for line in snapshot.split_inclusive('\n') {
        if line.trim_end().ends_with(boundary) {
            return Some(&snapshot[..length]);
        }
        length += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::before_hvc;

    #[test]
    fn replays_only_messages_before_the_hvc_registration() {
        let early =
            "<6>Booting Linux on physical CPU 0x0000000000\n<6>Kernel command line: console=hvc0\n";
        let snapshot = format!(
            "{early}<6>printk: legacy console [hvc0] enabled\n<6>virtio_blk virtio2: [vda]\n"
        );
        assert_eq!(before_hvc(&snapshot), Some(early));
        assert_eq!(before_hvc(early), None);
    }
}
