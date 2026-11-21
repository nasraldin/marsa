//! Rotate private JSONL logs without blocking the daemon's Tokio runtime.

use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use marsa_core::paths::MarsaPaths;
use nix::fcntl::OFlag;
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};

use super::{DaemonError, files};

const MAX_LOG_FILES: usize = 7;
const LOG_PREFIX: &str = "marsad";
const LOG_SUFFIX: &str = "jsonl";

pub(super) fn initialize(paths: &MarsaPaths, uid: u32) -> Result<WorkerGuard, DaemonError> {
    let directory = paths.logs_dir();
    let current = paths.daemon_log();
    verify_log_paths(&directory, &current, uid)?;
    let name = current
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("invalid daemon log filename"))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix(LOG_SUFFIX)
        .latest_symlink(name)
        .max_log_files(MAX_LOG_FILES)
        .build(directory)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_writer(writer)
        .try_init()?;
    Ok(guard)
}

fn verify_log_paths(directory: &Path, current: &Path, uid: u32) -> io::Result<()> {
    match fs::symlink_metadata(current) {
        Ok(metadata) if metadata.is_symlink() && metadata.uid() == uid => {
            let target = fs::read_link(current)?;
            if target.parent() != Some(directory)
                || !target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_log_name)
            {
                return Err(unexpected(current));
            }
        }
        Ok(_) => return Err(unexpected(current)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(LOG_PREFIX) || !name.ends_with(LOG_SUFFIX) {
            continue;
        }
        // The appender may prune matching regular files. Authorize the whole
        // matching set, and refuse a symlink where it would open a dated log.
        let path = entry.path();
        if !is_log_name(name) {
            return Err(unexpected(&path));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(&path)?;
        if !files::is_owned_file(&file.metadata()?, uid) {
            return Err(unexpected(&path));
        }
        files::set_file_mode(&file, 0o600)?;
    }
    Ok(())
}

fn is_log_name(name: &str) -> bool {
    let date = name
        .strip_prefix("marsad.")
        .and_then(|name| name.strip_suffix(".jsonl"));
    let Some(date) = date else { return false };
    date.len() == 10
        && date.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}

fn unexpected(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("unexpected daemon log path {}", path.display()),
    )
}
