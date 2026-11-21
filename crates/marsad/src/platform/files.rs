//! Authorize daemon-owned paths and hold a stable lock through runtime cleanup.

use std::{
    fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions, TryLockError},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
};

use marsa_core::{
    paths::MarsaPaths,
    settings::{MAX_SETTINGS_BYTES, Settings},
};
use nix::fcntl::OFlag;

const PRIVATE_DIR: u32 = 0o700;
const PRIVATE_FILE: u32 = 0o600;
const PRIVATE_MODE_BITS: u32 = 0o7777;

pub(super) struct Instance {
    // Keep the inode on disk both during the lock and after release. A waiting
    // launcher must always contend on the same inode.
    _lock: File,
    socket: Option<OwnedPath>,
    pid: Option<OwnedPath>,
}

impl Instance {
    pub(super) fn acquire(paths: &MarsaPaths, uid: u32) -> io::Result<Self> {
        private_dir(paths.root(), uid)?;
        private_dir(&paths.run_dir(), uid)?;
        let path = paths.daemon_lock_file();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(PRIVATE_FILE)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(&path)?;
        if !is_owned_file(&lock.metadata()?, uid) {
            return Err(unexpected(&path));
        }
        match lock.try_lock() {
            Ok(()) => (),
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "another marsad instance is already running",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        set_file_mode(&lock, PRIVATE_FILE)?;
        for path in [paths.vm_dir(), paths.logs_dir(), paths.state_dir()] {
            private_dir(&path, uid)?;
        }
        Ok(Self {
            _lock: lock,
            socket: None,
            pid: None,
        })
    }

    pub(super) fn publish(&mut self, paths: &MarsaPaths, uid: u32) -> io::Result<UnixListener> {
        let socket = paths.daemon_socket();
        let pid = paths.daemon_pid_file();
        remove_stale(&socket, |entry| {
            entry.file_type().is_socket() && entry.uid() == uid
        })?;
        remove_stale(&pid, |entry| is_owned_file(entry, uid))?;
        let listener = UnixListener::bind(&socket)?;
        let metadata = fs::symlink_metadata(&socket)?;
        if !metadata.file_type().is_socket() || metadata.uid() != uid {
            return Err(unexpected(&socket));
        }
        self.socket = Some(OwnedPath::new(socket.clone(), &metadata));
        fs::set_permissions(&socket, Permissions::from_mode(PRIVATE_FILE))?;
        listener.set_nonblocking(true)?;

        let mut process = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE)
            .open(&pid)?;
        self.pid = Some(OwnedPath::new(pid, &process.metadata()?));
        writeln!(process, "{}", std::process::id())?;
        Ok(listener)
    }

    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        let socket = self.socket.take().map_or(Ok(()), OwnedPath::remove);
        let pid = self.pid.take().map_or(Ok(()), OwnedPath::remove);
        socket.and(pid)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

pub(super) fn read_settings(paths: &MarsaPaths, uid: u32) -> io::Result<Settings> {
    let file = paths.config_file();
    let open = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(&file);
    let input = match open {
        Ok(input) => input,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => {
            return Err(io::Error::other(format!(
                "could not open settings: {error}"
            )));
        }
    };
    let metadata = input.metadata()?;
    if !is_owned_file(&metadata, uid) {
        return Err(unexpected(&file));
    }
    let mut text = String::new();
    input
        .take(MAX_SETTINGS_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|error| io::Error::other(format!("could not read settings: {error}")))?;
    Settings::from_toml(&text)
        .map_err(|error| io::Error::other(format!("could not load settings: {error}")))
}

fn private_dir(path: &Path, uid: u32) -> io::Result<()> {
    match DirBuilder::new().mode(PRIVATE_DIR).create(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        result => result?,
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(path)
        .map_err(|error| {
            io::Error::other(format!(
                "could not open private directory {}: {error}",
                path.display()
            ))
        })?;
    if directory.metadata()?.uid() != uid {
        return Err(unexpected(path));
    }
    set_file_mode(&directory, PRIVATE_DIR)
}

pub(super) fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    if file.metadata()?.mode() & PRIVATE_MODE_BITS != mode {
        file.set_permissions(Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub(super) fn is_owned_file(metadata: &Metadata, uid: u32) -> bool {
    metadata.is_file() && metadata.uid() == uid && metadata.nlink() == 1
}

fn remove_stale(path: &Path, allowed: impl FnOnce(&Metadata) -> bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if allowed(&metadata) => OwnedPath::new(path.to_owned(), &metadata).remove(),
        Ok(_) => Err(unexpected(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn unexpected(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("unexpected owner or file type at {}", path.display()),
    )
}

struct OwnedPath {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl OwnedPath {
    fn new(path: PathBuf, metadata: &Metadata) -> Self {
        Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn remove(self) -> io::Result<()> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if (metadata.dev(), metadata.ino()) == (self.device, self.inode) => {
                fs::remove_file(&self.path)
            }
            Ok(_) => Err(unexpected(&self.path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}
