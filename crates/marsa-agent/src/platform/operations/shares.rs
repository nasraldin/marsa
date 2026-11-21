//! Restrict runtime mounts to the same exact boot paths PID 1 validated.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use marsa_core::guest_boot::{Settings, Share};
use marsa_proto::guest::v1::{MountShare, UnmountShare};
use nix::{
    errno::Errno,
    mount::{MsFlags, mount, umount},
};

use super::stats::read_bounded;

const SHARE_FLAGS: MsFlags = MsFlags::MS_NODEV.union(MsFlags::MS_NOSUID);

pub(super) fn attach(request: MountShare) -> io::Result<()> {
    let share = configured(&request.tag)?;
    if share.path.as_os_str() != request.guest_path.as_str() {
        return Err(invalid(
            "virtiofs destination does not match its configured path",
        ));
    }
    check_directories(&share.path)?;
    let mut flags = SHARE_FLAGS;
    flags.set(MsFlags::MS_RDONLY, request.read_only);
    if let Some(read_only) = mounted(&share)? {
        if read_only == request.read_only {
            return Ok(());
        }
        return mount(
            None::<&str>,
            &share.path,
            None::<&str>,
            flags | MsFlags::MS_REMOUNT,
            None::<&str>,
        )
        .map_err(Into::into);
    }
    mount(
        Some(share.tag.as_str()),
        &share.path,
        Some("virtiofs"),
        flags,
        None::<&str>,
    )?;
    Ok(())
}

pub(super) fn detach(request: UnmountShare) -> io::Result<()> {
    let share = configured(&request.tag)?;
    check_directories(&share.path)?;
    if mounted(&share)?.is_some() {
        match umount(&share.path) {
            Ok(()) => {}
            Err(Errno::EINVAL) if mounted(&share)?.is_none() => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn configured(tag: &str) -> io::Result<Share> {
    Settings::parse(&read_bounded("/proc/cmdline", 4096)?)?
        .shares
        .into_iter()
        .find(|share| share.tag == tag)
        .ok_or_else(|| invalid("virtiofs tag was not configured at boot"))
}

fn check_directories(path: &Path) -> io::Result<()> {
    let mut ancestor = PathBuf::from("/");
    for name in path.iter().skip(1) {
        ancestor.push(name);
        if !fs::symlink_metadata(&ancestor)?.is_dir() {
            return Err(invalid(
                "virtiofs destination has a non-directory or symlink ancestor",
            ));
        }
    }
    Ok(())
}

fn mounted(share: &Share) -> io::Result<Option<bool>> {
    let info = read_bounded("/proc/self/mountinfo", 4 * 1024 * 1024)?;
    match_mount(&info, share)
}

fn match_mount(info: &str, share: &Share) -> io::Result<Option<bool>> {
    let path = share
        .path
        .to_str()
        .ok_or_else(|| invalid("invalid virtiofs path"))?
        .replace('\\', "\\134")
        .replace(' ', "\\040");
    let mut entries = info.lines().filter_map(|line| {
        let (mount, filesystem) = line.split_once(" - ")?;
        let mut fields = mount.split_ascii_whitespace();
        (fields.nth(4)? == path).then(|| (fields.next(), filesystem))
    });
    let Some((Some(options), fs)) = entries.next() else {
        return Ok(None);
    };
    let mut fields = fs.split_ascii_whitespace();
    if fields.next() != Some("virtiofs")
        || fields.next() != Some(share.tag.as_str())
        || entries.next().is_some()
    {
        return Err(invalid("configured virtiofs path belongs to another mount"));
    }
    Ok(Some(options.split(',').any(|option| option == "ro")))
}

fn invalid(detail: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, detail)
}

#[cfg(test)]
mod tests {
    use marsa_core::guest_boot::Share;

    use super::match_mount;

    #[test]
    fn identifies_exact_mount_and_rejects_conflicts() {
        let share = Share {
            tag: "project".into(),
            path: "/Users/with space".into(),
        };
        let good = "22 12 0:6 / /Users/with\\040space ro,nosuid - virtiofs project rw";
        assert_eq!(match_mount(good, &share).unwrap(), Some(true));
        assert!(match_mount(&good.replace("virtiofs project", "virtiofs other"), &share).is_err());
        assert!(match_mount(&format!("{good}\n{good}"), &share).is_err());
        assert_eq!(match_mount("", &share).unwrap(), None);
    }
}
