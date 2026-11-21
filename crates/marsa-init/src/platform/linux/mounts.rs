//! Mount volatile guest state and configured host shares.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use nix::{
    errno::Errno,
    mount::{MsFlags, mount, umount},
};

use marsa_core::guest_boot::Share;

const UNPRIVILEGED: MsFlags = MsFlags::MS_NOSUID.union(MsFlags::MS_NODEV);
const NOEXEC: MsFlags = UNPRIVILEGED.union(MsFlags::MS_NOEXEC);

#[derive(Default)]
pub(super) struct Mounts {
    shares: Vec<PathBuf>,
    share_roots: Vec<PathBuf>,
}

impl Mounts {
    pub(super) fn system(&self) -> io::Result<()> {
        filesystem("proc", "/proc", "proc", NOEXEC, None)?;
        // The kernel mounts devtmpfs before PID 1 when booting the EROFS disk.
        let mounts = fs::read_to_string("/proc/self/mountinfo")?;
        let existing_device_fs = mounts.lines().find_map(|line| {
            let (mount, filesystem) = line.split_once(" - ")?;
            (mount.split_ascii_whitespace().nth(4) == Some("/dev"))
                .then(|| filesystem.split_ascii_whitespace().next())
                .flatten()
        });
        let (device_flags, device_options) = match existing_device_fs {
            Some("devtmpfs") => (MsFlags::MS_REMOUNT | MsFlags::MS_NOSUID, None),
            Some(_) => return Err(io::Error::other("unexpected filesystem mounted at /dev")),
            None => (MsFlags::MS_NOSUID, Some("mode=0755")),
        };
        filesystem("devtmpfs", "/dev", "devtmpfs", device_flags, device_options)?;
        filesystem("sysfs", "/sys", "sysfs", NOEXEC, None)?;
        filesystem("cgroup2", "/sys/fs/cgroup", "cgroup2", NOEXEC, None)?;
        filesystem("tmpfs", "/run", "tmpfs", UNPRIVILEGED, Some("mode=0755"))?;
        filesystem("tmpfs", "/tmp", "tmpfs", UNPRIVILEGED, Some("mode=1777"))?;
        fs::create_dir("/run/etc-upper")?;
        fs::create_dir("/run/etc-work")?;
        filesystem(
            "overlay",
            "/etc",
            "overlay",
            UNPRIVILEGED,
            Some("lowerdir=/etc,upperdir=/run/etc-upper,workdir=/run/etc-work"),
        )
    }

    pub(super) fn share_all(&mut self, shares: &[Share]) -> io::Result<()> {
        for share in shares {
            let root = share
                .path
                .ancestors()
                .find(|path| path.parent() == Some(Path::new("/")))
                .ok_or_else(|| io::Error::other("share is missing its top-level mount path"))?;
            if root != Path::new("/tmp") && !self.share_roots.iter().any(|mounted| mounted == root)
            {
                ensure_path(root)?;
                filesystem("tmpfs", root, "tmpfs", UNPRIVILEGED, Some("mode=0755"))?;
                self.share_roots.push(root.to_owned());
            }
            ensure_path(&share.path)?;
            filesystem(&share.tag, &share.path, "virtiofs", UNPRIVILEGED, None)?;
            self.shares.push(share.path.clone());
        }
        Ok(())
    }

    pub(super) fn unmount_shares(&mut self) -> io::Result<()> {
        let mut first_error = None;
        for path in self
            .shares
            .drain(..)
            .rev()
            .chain(self.share_roots.drain(..).rev())
        {
            if let Err(error) = umount(path.as_path())
                && error != Errno::EINVAL
            {
                first_error.get_or_insert_with(|| {
                    io::Error::other(format!("{}: {error}", path.display()))
                });
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn filesystem(
    source: &str,
    target: impl AsRef<Path>,
    kind: &str,
    flags: MsFlags,
    data: Option<&str>,
) -> io::Result<()> {
    let target = target.as_ref();
    mount(Some(source), target, Some(kind), flags, data)
        .map_err(|error| io::Error::other(format!("mount {} ({kind}): {error}", target.display())))
}

fn ensure_path(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::from("/");
    for part in path.components() {
        let Component::Normal(name) = part else {
            continue;
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "share path is not a directory: {}",
                    current.display()
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
