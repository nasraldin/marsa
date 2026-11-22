//! Anchor factory storage operations to private, already-open directories.

use std::{
    fs::{File, Metadata},
    io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat},
    sys::stat::{Mode, mkdirat},
    unistd::{UnlinkatFlags, unlinkat},
};
use serde::{Deserialize, Serialize};

const DIRECTORIES: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);
const FILES: OFlag = OFlag::O_NOFOLLOW
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inode {
    device: u64,
    inode: u64,
}

impl Inode {
    pub(super) fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

pub(super) struct Directory {
    pub(super) file: File,
    uid: u32,
}

impl Directory {
    pub(super) fn root(path: &Path, uid: u32) -> io::Result<Self> {
        Self::verify(File::from(open(path, DIRECTORIES, Mode::empty())?), uid)
    }

    pub(super) fn child(&self, name: &str) -> io::Result<Option<Self>> {
        match openat(&self.file, name, DIRECTORIES, Mode::empty()) {
            Ok(file) => Self::verify(File::from(file), self.uid).map(Some),
            Err(Errno::ENOENT) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn create_directory(&self, name: &str) -> io::Result<Self> {
        mkdirat(&self.file, name, Mode::S_IRWXU)?;
        let directory = self.child(name)?.ok_or_else(invalid)?;
        self.sync()?;
        Ok(directory)
    }

    fn verify(file: File, uid: u32) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
            return Err(invalid());
        }
        Ok(Self { file, uid })
    }

    pub(super) fn inode(&self) -> io::Result<Inode> {
        Ok(Inode::from(&self.file.metadata()?))
    }

    pub(super) fn same_device(&self, other: &Self) -> io::Result<bool> {
        Ok(self.file.metadata()?.dev() == other.file.metadata()?.dev())
    }

    pub(super) fn entry(&self, name: &str, writable: bool) -> io::Result<Option<File>> {
        let access = if writable {
            OFlag::O_RDWR
        } else {
            OFlag::O_RDONLY
        };
        let file = match openat(&self.file, name, FILES | access, Mode::empty()) {
            Ok(file) => File::from(file),
            Err(Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != self.uid || metadata.nlink() != 1 {
            return Err(invalid());
        }
        Ok(Some(file))
    }

    pub(super) fn create_file(&self, name: &str) -> io::Result<File> {
        let file = File::from(openat(
            &self.file,
            name,
            FILES | OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?);
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }

    pub(super) fn entry_inode(&self, name: &str) -> io::Result<Option<Inode>> {
        self.entry(name, false)?
            .map(|file| file.metadata().map(|meta| Inode::from(&meta)))
            .transpose()
    }

    pub(super) fn remove(&self, name: &str, expected: Inode) -> io::Result<()> {
        if self.entry_inode(name)? != Some(expected) {
            return Err(invalid());
        }
        unlinkat(&self.file, name, UnlinkatFlags::NoRemoveDir)?;
        self.sync()
    }

    pub(super) fn remove_directory(&self, name: &str, expected: Inode) -> io::Result<()> {
        if self
            .child(name)?
            .map(|directory| directory.inode())
            .transpose()?
            != Some(expected)
        {
            return Err(invalid());
        }
        unlinkat(&self.file, name, UnlinkatFlags::RemoveDir)?;
        self.sync()
    }

    pub(super) fn move_entry(
        &self,
        name: &str,
        destination: &Self,
        target: &str,
        expected: Inode,
    ) -> io::Result<()> {
        if self.entry_inode(name)? != Some(expected) {
            return Err(invalid());
        }
        move_exclusive(&self.file, name, &destination.file, target)?;
        destination.sync()?;
        self.sync()
    }

    pub(super) fn move_directory(
        &self,
        name: &str,
        target: &str,
        expected: Inode,
    ) -> io::Result<()> {
        if self
            .child(name)?
            .map(|directory| directory.inode())
            .transpose()?
            != Some(expected)
        {
            return Err(invalid());
        }
        move_exclusive(&self.file, name, &self.file, target)?;
        self.sync()
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }
}

pub(super) fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unexpected private factory storage member",
    )
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn move_exclusive(from: &File, source: &str, to: &File, destination: &str) -> io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let source = CString::new(source)?;
    let destination = CString::new(destination)?;
    // SAFETY: The directory descriptors remain open and both names are live C strings.
    let status = unsafe {
        nix::libc::renameatx_np(
            from.as_raw_fd(),
            source.as_ptr(),
            to.as_raw_fd(),
            destination.as_ptr(),
            nix::libc::RENAME_EXCL,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn move_exclusive(from: &File, source: &str, to: &File, destination: &str) -> io::Result<()> {
    use nix::fcntl::{RenameFlags, renameat2};
    renameat2(from, source, to, destination, RenameFlags::RENAME_NOREPLACE).map_err(Into::into)
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
fn move_exclusive(_: &File, _: &str, _: &File, _: &str) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}
