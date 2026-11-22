//! Access the grants file relative to owner-only directory descriptors.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

use marsa_core::paths::MarsaPaths;
use nix::{
    errno::Errno,
    fcntl::{OFlag, openat, renameat},
    sys::stat::Mode,
    unistd::{UnlinkatFlags, unlinkat},
};

const FILE: &str = "grants.json";
const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

pub(super) fn read(paths: &MarsaPaths, uid: u32, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let directory = private_state(paths, uid)?;
    let Some(file) = existing(&directory, uid)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(limit as u64).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

pub(super) fn write(paths: &MarsaPaths, uid: u32, bytes: &[u8]) -> io::Result<()> {
    let directory = private_state(paths, uid)?;
    existing(&directory, uid)?;
    let nonce = getrandom::u64().map_err(io::Error::other)?;
    let staging = format!(".grants.{nonce:016x}");
    let file = openat(
        &directory,
        staging.as_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?;
    let mut file = File::from(file);
    let result = (|| {
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        renameat(&directory, staging.as_str(), &directory, FILE)?;
        if let Err(error) = directory.sync_all() {
            tracing::warn!(kind = ?error.kind(), "agent grants directory sync failed");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = unlinkat(&directory, staging.as_str(), UnlinkatFlags::NoRemoveDir);
    }
    result
}

fn existing(directory: &File, uid: u32) -> io::Result<Option<File>> {
    let file = match openat(
        directory,
        FILE,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(Some(file))
}

fn private_state(paths: &MarsaPaths, uid: u32) -> io::Result<File> {
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS.bits())
        .open(paths.root())?;
    check_directory(&root, uid)?;
    let state = File::from(openat(&root, "state", DIRECTORY_FLAGS, Mode::empty())?);
    check_directory(&state, uid)?;
    Ok(state)
}

fn check_directory(directory: &File, uid: u32) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}
