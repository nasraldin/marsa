//! Probe, validate, and resize the guest's whole virtio block device.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use nix::fcntl::OFlag;
use nix::sys::stat::{major, minor};

use super::MOUNT_TARGETS;

/// Whole virtio data disk presented after the root device.
pub(super) const DEVICE_PATH: &str = "/dev/vdb";
const SYSFS_DEVICE: &str = "/sys/block/vdb";

/// Ensure ext4 is ready to mount and report whether its capacity can grow.
pub(super) fn prepare() -> io::Result<bool> {
    let (device, bytes) = open_device()?;
    let probe = disk_tool("/sbin/blkid")
        .args(["-p", "-o", "export", DEVICE_PATH])
        .output()?;
    match probe.status.code() {
        Some(0) if has_plain_ext4(&probe.stdout) => {
            let status = disk_tool("/sbin/e2fsck")
                .args(["-p", DEVICE_PATH])
                .status()?;
            check_e2fsck_status(status.code())?;
        }
        Some(2) if is_blank(&device, bytes)? => {
            require_success(
                "mkfs.ext4",
                disk_tool("/sbin/mkfs.ext4").arg(DEVICE_PATH).status()?,
            )?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{DEVICE_PATH} is neither blank nor an unpartitioned ext4 filesystem (blkid {})",
                    probe.status
                ),
            ));
        }
    }
    let mut superblock = [0; 1024];
    device.read_exact_at(&mut superblock, 1024)?;
    needs_growth(&superblock, bytes)
}

/// Expand a checked, unmounted ext4 filesystem to the device capacity.
pub(super) fn grow() -> io::Result<()> {
    // Offline resize requires a full check after the last mount.
    let status = disk_tool("/sbin/e2fsck")
        .args(["-f", "-p", DEVICE_PATH])
        .status()?;
    check_e2fsck_status(status.code())?;
    require_success(
        "resize2fs",
        disk_tool("/usr/sbin/resize2fs").arg(DEVICE_PATH).status()?,
    )
}

fn open_device() -> io::Result<(File, u64)> {
    let metadata = fs::symlink_metadata(DEVICE_PATH)?;
    let device_id = format!("{}:{}", major(metadata.rdev()), minor(metadata.rdev()));
    let sysfs = Path::new(SYSFS_DEVICE);
    if !metadata.file_type().is_block_device()
        || fs::read_to_string(sysfs.join("dev"))?.trim() != device_id
        || fs::read_link(sysfs.join("device/driver"))?.file_name() != Some("virtio_blk".as_ref())
        || fs::read_to_string(sysfs.join("ro"))?.trim() != "0"
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{DEVICE_PATH} must be a writable secondary virtio block device"),
        ));
    }
    let mount_info = fs::read_to_string("/proc/self/mountinfo")?;
    for line in mount_info.lines() {
        let mut fields = line.split_whitespace();
        let mounted_device = fields.nth(2);
        let mount_point = fields.nth(1);
        if mounted_device == Some(device_id.as_str())
            || mount_point.is_some_and(|point| MOUNT_TARGETS.contains(&point))
        {
            return Err(io::Error::other(format!(
                "{DEVICE_PATH} or a data mount point is already mounted"
            )));
        }
    }
    let bytes = fs::read_to_string(sysfs.join("size"))?
        .trim()
        .parse::<u64>()
        .ok()
        // Linux reports the size of a block device in 512-byte sectors.
        .and_then(|sectors| sectors.checked_mul(512))
        .filter(|bytes| *bytes != 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid data disk capacity"))?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(DEVICE_PATH)?;
    let opened = file.metadata()?;
    if !opened.file_type().is_block_device() || opened.rdev() != metadata.rdev() {
        return Err(io::Error::other(format!(
            "{DEVICE_PATH} changed during verification"
        )));
    }
    Ok((file, bytes))
}

fn disk_tool(path: &str) -> Command {
    let mut command = Command::new(path);
    command.env_clear().env("LC_ALL", "C").stdin(Stdio::null());
    command
}

fn require_success(tool: &str, status: ExitStatus) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{tool} failed for {DEVICE_PATH}: {status}"
        )))
    }
}

fn has_plain_ext4(tags: &[u8]) -> bool {
    let mut filesystem = None;
    for tag in tags.split(|byte| *byte == b'\n') {
        if tag.starts_with(b"PTTYPE=") || tag.starts_with(b"PART_ENTRY_") {
            return false;
        }
        if let Some(value) = tag.strip_prefix(b"TYPE=")
            && filesystem.replace(value).is_some()
        {
            return false;
        }
    }
    filesystem == Some(b"ext4")
}

fn is_blank(mut device: impl Read, mut bytes: u64) -> io::Result<bool> {
    // A probe without a recognized signature can still contain user data.
    let mut buffer = vec![0; 1024 * 1024];
    while bytes != 0 {
        let chunk = bytes.min(buffer.len() as u64) as usize;
        device.read_exact(&mut buffer[..chunk])?;
        if buffer[..chunk].iter().any(|byte| *byte != 0) {
            return Ok(false);
        }
        bytes -= chunk as u64;
    }
    Ok(true)
}

fn check_e2fsck_status(status: Option<i32>) -> io::Result<()> {
    // e2fsck -p may fix safe errors automatically. A required reboot or any
    // unresolved problem prevents the engine from starting.
    match status {
        Some(0 | 1) => Ok(()),
        Some(code) => Err(io::Error::other(format!(
            "e2fsck failed for {DEVICE_PATH} with status {code}"
        ))),
        None => Err(io::Error::other(format!(
            "e2fsck was interrupted for {DEVICE_PATH}"
        ))),
    }
}

fn needs_growth(superblock: &[u8; 1024], bytes: u64) -> io::Result<bool> {
    // Read exact ext4 geometry. statfs subtracts reserved metadata blocks
    // from total capacity, while unguarded resize2fs can shrink offline.
    let word = |offset| u32::from_le_bytes(std::array::from_fn(|index| superblock[offset + index]));
    let log_block_size = word(0x18);
    if superblock[0x38..0x3a] != [0x53, 0xef] || log_block_size > 6 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid ext4 superblock",
        ));
    }
    let block_size = 1024_u64 << log_block_size;
    let mut blocks = u64::from(word(0x04));
    if word(0x60) & 0x80 != 0 {
        // EXT4_FEATURE_INCOMPAT_64BIT makes the high count valid.
        blocks |= u64::from(word(0x150)) << 32;
    }
    let device_blocks = bytes / block_size;
    if blocks == 0 || blocks > device_blocks {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ext4 size exceeds the data disk capacity or is zero",
        ));
    }
    Ok(blocks < device_blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unrecognized_data_beyond_the_first_megabyte() {
        let mut disk = vec![0; 2 * 1024 * 1024];
        assert!(is_blank(disk.as_slice(), disk.len() as u64).unwrap());
        disk[1024 * 1024 + 17] = 1;
        assert!(!is_blank(disk.as_slice(), disk.len() as u64).unwrap());
    }

    #[test]
    fn rejects_a_short_disk_read() {
        assert_eq!(
            is_blank(&[0; 4][..], 8).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn rejects_partitioned_and_ambiguous_filesystems() {
        assert!(has_plain_ext4(
            b"DEVNAME=/dev/vdb\nTYPE=ext4\nUSAGE=filesystem\n"
        ));
        assert!(!has_plain_ext4(b"TYPE=ext4\nPTTYPE=gpt\n"));
        assert!(!has_plain_ext4(b"TYPE=ext4\nTYPE=xfs\n"));
        assert!(!has_plain_ext4(b"TYPE=ext3\n"));
    }

    #[test]
    fn grows_only_to_add_whole_ext4_blocks() {
        let mut superblock = [0; 1024];
        superblock[0x04..0x08].copy_from_slice(&16_384_u32.to_le_bytes());
        superblock[0x18..0x1c].copy_from_slice(&2_u32.to_le_bytes());
        superblock[0x38..0x3a].copy_from_slice(&[0x53, 0xef]);
        assert!(!needs_growth(&superblock, 67_109_376).unwrap());
        assert!(needs_growth(&superblock, 67_112_960).unwrap());
        assert!(needs_growth(&superblock, 67_108_352).is_err());
    }

    #[test]
    fn checks_that_fsck_completed_without_unresolved_errors() {
        assert!(check_e2fsck_status(Some(0)).is_ok());
        assert!(check_e2fsck_status(Some(1)).is_ok());
        for status in [Some(2), Some(4), Some(5), Some(8), None] {
            assert!(check_e2fsck_status(status).is_err());
        }
    }
}
