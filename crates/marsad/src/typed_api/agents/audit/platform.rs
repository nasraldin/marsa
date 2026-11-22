//! Work relative to owner-only directory handles and never follow audit file aliases.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom as SeekAt},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, openat, renameat},
    sys::stat::Mode,
};

const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

pub(super) fn state(root: &Path, uid: u32) -> io::Result<File> {
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(DIR_FLAGS.bits())
        .open(root)?;
    private_directory(&root, uid)?;
    let state = File::from(openat(&root, "state", DIR_FLAGS, Mode::empty())?);
    private_directory(&state, uid)?;
    Ok(state)
}

fn private_directory(directory: &File, uid: u32) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if metadata.uid() == uid && metadata.mode() & 0o7777 == 0o700 {
        Ok(())
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

pub(super) fn existing(
    directory: &File,
    name: &str,
    uid: u32,
    limit: u64,
) -> io::Result<Option<File>> {
    let flags =
        OFlag::O_RDWR | OFlag::O_APPEND | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let descriptor = match openat(directory, name, flags, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != 0o600
        || metadata.len() > limit
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(Some(file))
}

pub(super) fn create(directory: &File, name: &str) -> io::Result<File> {
    let flags = OFlag::O_WRONLY
        | OFlag::O_APPEND
        | OFlag::O_CREAT
        | OFlag::O_EXCL
        | OFlag::O_NOFOLLOW
        | OFlag::O_CLOEXEC;
    let file = File::from(openat(
        directory,
        name,
        flags,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?);
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    directory.sync_all()?;
    Ok(file)
}

pub(super) fn clean_end(file: &mut File, size: u64) -> io::Result<bool> {
    if size == 0 {
        return Ok(true);
    }
    file.seek(SeekAt::End(-1))?;
    let mut last = [0; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

pub(super) fn rotate(directory: &File, names: &[&str], uid: u32, limit: u64) -> io::Result<()> {
    for name in names {
        existing(directory, name, uid, limit)?;
    }
    for pair in names.windows(2).rev() {
        match renameat(directory, pair[0], directory, pair[1]) {
            Ok(()) | Err(Errno::ENOENT) => {}
            Err(error) => return Err(error.into()),
        }
    }
    directory.sync_all()
}
