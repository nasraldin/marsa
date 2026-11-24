//! Snapshot and atomically replace the private configuration through its directory descriptor.

use std::{
    fs::{File, Metadata, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, openat, renameat},
    sys::stat::Mode,
    unistd::{UnlinkatFlags, unlinkat},
};
use thiserror::Error;

#[cfg(test)]
mod tests;

const CONFIG: &str = "config.toml";
const PRIVATE_FILE: Mode = Mode::S_IRUSR.union(Mode::S_IWUSR);

/// Keeps the original directory and member inodes alive until the challenge is consumed.
pub(in super::super) struct PreparedConfig {
    directory: File,
    member: Option<PreparedMember>,
}

struct PreparedMember {
    file: File,
    mode: u32,
    contents: Vec<u8>,
}

#[derive(Debug, Error)]
pub(super) enum WriteError {
    #[error("the configuration changed since reset preparation")]
    Changed,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub(in super::super) fn prepare(root: &Path, uid: u32) -> io::Result<PreparedConfig> {
    let directory = open_directory(root, uid, true)?;
    let member = open_member(&directory, CONFIG, uid)?
        .map(|file| {
            Ok::<_, io::Error>(PreparedMember {
                mode: file.metadata()?.mode(),
                contents: read_member(&file)?,
                file,
            })
        })
        .transpose()?;
    Ok(PreparedConfig { directory, member })
}

pub(super) fn persist(
    root: &Path,
    uid: u32,
    text: &str,
    expected: Option<&PreparedConfig>,
) -> Result<(), WriteError> {
    persist_with(root, uid, text, expected, || Ok(()))
}

fn persist_with(
    root: &Path,
    uid: u32,
    text: &str,
    expected: Option<&PreparedConfig>,
    after_stage_sync: impl FnOnce() -> io::Result<()>,
) -> Result<(), WriteError> {
    let directory = checked_directory(root, uid, expected)?;
    let nonce = getrandom::u64().map_err(io::Error::other)?;
    let staging = format!(".settings.{nonce:016x}");
    let mut file = File::from(
        openat(
            &directory,
            staging.as_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            PRIVATE_FILE,
        )
        .map_err(io::Error::from)?,
    );
    let created = inode(&file.metadata()?);
    let owns_stage = || -> io::Result<bool> {
        let Some(named) = open_member(&directory, &staging, uid)? else {
            return Ok(false);
        };
        Ok(inode(&named.metadata()?) == created)
    };
    let result = (|| {
        file.set_permissions(Permissions::from_mode(0o600))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        after_stage_sync()?;
        if expected.is_some() {
            checked_directory(root, uid, expected)?;
        }
        if !owns_stage()? {
            return Err(WriteError::Changed);
        }
        renameat(&directory, staging.as_str(), &directory, CONFIG).map_err(io::Error::from)?;

        // The rename commits the complete document in the running daemon.
        if let Err(error) = directory.sync_all() {
            tracing::warn!(kind = ?error.kind(), "settings directory sync failed");
        }
        Ok(())
    })();
    if result.is_err() && owns_stage().unwrap_or(false) {
        let _ = unlinkat(&directory, staging.as_str(), UnlinkatFlags::NoRemoveDir);
    }
    result
}

fn checked_directory(
    root: &Path,
    uid: u32,
    expected: Option<&PreparedConfig>,
) -> Result<File, WriteError> {
    if let Some(expected) = expected {
        let directory = open_directory(root, uid, true).map_err(|_| WriteError::Changed)?;
        let member = open_member(&directory, CONFIG, uid).map_err(|_| WriteError::Changed)?;
        expected.verify(&directory, member.as_ref())?;
        return Ok(directory);
    }
    let directory = open_directory(root, uid, false)?;
    open_member(&directory, CONFIG, uid)?;
    Ok(directory)
}

impl PreparedConfig {
    fn verify(&self, directory: &File, member: Option<&File>) -> Result<(), WriteError> {
        if inode(&self.directory.metadata()?) != inode(&directory.metadata()?) {
            return Err(WriteError::Changed);
        }
        match (&self.member, member) {
            (None, None) => Ok(()),
            (Some(expected), Some(member)) => {
                let metadata = member.metadata()?;
                if inode(&expected.file.metadata()?) == inode(&metadata)
                    && expected.mode == metadata.mode()
                    && expected.contents == read_member(member)?
                {
                    return Ok(());
                }
                Err(WriteError::Changed)
            }
            _ => Err(WriteError::Changed),
        }
    }
}

fn open_directory(root: &Path, uid: u32, require_private: bool) -> io::Result<File> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(root)?;
    let metadata = directory.metadata()?;
    if metadata.uid() != uid || (require_private && metadata.mode() & 0o7777 != 0o700) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(directory)
}

fn open_member(directory: &File, name: &str, uid: u32) -> io::Result<Option<File>> {
    let member = match openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = member.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.nlink() != 1 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(Some(member))
}

fn read_member(member: &File) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    member
        .take(marsa_core::settings::MAX_SETTINGS_BYTES as u64 + 1)
        .read_to_end(&mut contents)?;
    if contents.len() > marsa_core::settings::MAX_SETTINGS_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(contents)
}

fn inode(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}
