//! Observe Linux inotify, ownership, and extended attributes on the shared mount.

mod attributes;

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use nix::unistd::{Gid, Uid, getgid, getuid, setgid, setgroups, setuid};

pub(super) use attributes::xattrs;

pub(super) fn create_unprivileged(root: &Path) -> io::Result<()> {
    setgroups(&[])?;
    setgid(Gid::from_raw(12345))?;
    setuid(Uid::from_raw(12345))?;
    println!("VF_GUEST_CREATOR uid={} gid={}", getuid(), getgid());
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o640)
        .open(root.join("guest-unprivileged"))?
        .write_all(b"guest-unprivileged\n")
}

pub(super) fn watch(root: &Path) -> io::Result<()> {
    let flags = AddWatchFlags::IN_CREATE
        | AddWatchFlags::IN_MODIFY
        | AddWatchFlags::IN_CLOSE_WRITE
        | AddWatchFlags::IN_MOVED_FROM
        | AddWatchFlags::IN_MOVED_TO;
    let watcher = Inotify::init(InitFlags::IN_CLOEXEC | InitFlags::IN_NONBLOCK)?;
    let guest_path = root.join("guest-edit");
    fs::write(&guest_path, b"initial\n")?;
    let directory = watcher.add_watch(root, flags)?;
    let guest_file = watcher.add_watch(&guest_path, flags)?;
    let host_file = watcher.add_watch(&root.join("host-edit"), flags)?;

    OpenOptions::new()
        .append(true)
        .open(guest_path)?
        .write_all(b"guest-control\n")?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("watch-ready"))?
        .sync_all()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut guest = 0;
    let mut guest_inode = 0;
    let mut host_inode = 0;
    let mut host_edit = 0;
    let mut host_rename = 0;
    while Instant::now() < deadline {
        match watcher.read_events() {
            Ok(events) => {
                for event in events {
                    if event.wd == guest_file {
                        guest_inode += 1;
                    } else if event.wd == host_file {
                        host_inode += 1;
                    } else if event.wd == directory {
                        match event.name.as_deref().and_then(|name| name.to_str()) {
                            Some("guest-edit") => guest += 1,
                            Some("host-edit") => host_edit += 1,
                            Some("host-renamed") => host_rename += 1,
                            _ => {}
                        }
                    }
                }
            }
            Err(Errno::EAGAIN) => (),
            Err(error) => return Err(error.into()),
        }
        thread::sleep(Duration::from_millis(20));
    }
    if guest == 0 || guest_inode == 0 {
        return Err(io::Error::other(
            "guest-side inotify positive control did not fire",
        ));
    }
    if fs::read(root.join("host-edit"))? != b"initial\nhost-edit\n"
        || fs::read(root.join("host-renamed"))? != b"host-rename\n"
    {
        return Err(io::Error::other(
            "host edits were not visible after the watch window",
        ));
    }

    println!(
        "VF_WATCH window_ms=5000 guest_dir_events={guest} guest_file_events={guest_inode} host_dir_edit_events={host_edit} host_file_edit_events={host_inode} host_rename_events={host_rename} host_contents_visible=true"
    );
    Ok(())
}
