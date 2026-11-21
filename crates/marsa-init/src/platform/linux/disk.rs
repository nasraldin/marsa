//! Validates and mounts the guest's persistent engine disk.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

use nix::mount::{self, MsFlags};
use nix::unistd::syncfs;

mod device;
mod schema;

const DATA_DIR: &str = "/var/lib/marsa";
const ENGINE_DIRS: [(&str, &str); 2] = [
    ("docker", "/var/lib/docker"),
    ("containerd", "/var/lib/containerd"),
];
const MOUNT_TARGETS: [&str; 3] = [DATA_DIR, ENGINE_DIRS[0].1, ENGINE_DIRS[1].1];

/// Prepare persistent ext4 storage before starting any guest service.
pub(super) fn prepare() -> io::Result<()> {
    if std::process::id() != 1 {
        return Err(io::Error::other("data disk setup requires guest PID 1"));
    }
    for target in MOUNT_TARGETS {
        checked_directory(Path::new(target))?;
    }
    let grow = device::prepare()?;

    mount_data()?;
    let mut mounted = vec![DATA_DIR];
    let result = prepare_mounted_disk(grow, &mut mounted);
    if result.is_err()
        && let Err(error) = unmount_targets(&mounted)
    {
        eprintln!("marsa-init: data disk rollback: {error}");
    }
    result
}

fn mount_data() -> io::Result<()> {
    mount::mount(
        Some(device::DEVICE_PATH),
        DATA_DIR,
        Some("ext4"),
        MsFlags::empty(),
        None::<&str>,
    )?;
    Ok(())
}

fn prepare_mounted_disk(grow: bool, mounted: &mut Vec<&'static str>) -> io::Result<()> {
    let data_dir = Path::new(DATA_DIR);
    checked_directory(data_dir)?;
    schema::check_or_create(data_dir)?;
    if grow {
        // The guest kernel disallows writable block-device opens while the
        // filesystem is mounted. Resize offline after validating the schema.
        mount::umount(DATA_DIR)?;
        mounted.pop();
        device::grow()?;
        mount_data()?;
        mounted.push(DATA_DIR);
    }
    for (name, target) in ENGINE_DIRS {
        let backing = data_dir.join(name);
        match fs::DirBuilder::new().mode(0o700).create(&backing) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            result => result?,
        }
        checked_directory(&backing)?;
        mount::mount(
            Some(backing.as_path()),
            target,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        )?;
        mounted.push(target);
    }
    Ok(())
}

fn checked_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} must be a root-owned directory writable only by root",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Flush persistent writes and remove engine binds before the ext4 mount.
pub(super) fn unmount() -> io::Result<()> {
    let mount_info = fs::read_to_string("/proc/self/mountinfo")?;
    let targets: Vec<_> = MOUNT_TARGETS
        .into_iter()
        .filter(|target| {
            mount_info
                .lines()
                .any(|line| line.split_whitespace().nth(4) == Some(*target))
        })
        .collect();
    unmount_targets(&targets)
}

fn unmount_targets(targets: &[&str]) -> io::Result<()> {
    let mut result = Ok(());
    if targets.contains(&DATA_DIR) {
        result = File::open(DATA_DIR).and_then(|file| syncfs(file).map_err(io::Error::from));
    }
    for target in targets.iter().rev() {
        if let Err(error) = mount::umount(*target) {
            let error = io::Error::other(format!("unmount {target}: {error}"));
            if result.is_ok() {
                result = Err(error);
            } else {
                eprintln!("marsa-init: {error}");
            }
        }
    }
    result
}
