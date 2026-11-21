//! Create or grow the private sparse image without replacing stored guest data.

use std::{
    ffi::{CStr, CString},
    fs::{File, Metadata, OpenOptions, Permissions, TryLockError},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
};

use marsa_core::paths::MarsaPaths;
use nix::{
    fcntl::{OFlag, openat},
    libc,
    sys::stat::Mode,
    unistd::{UnlinkatFlags, unlinkat},
};

use super::{super::files::is_owned_file, SECTOR_BYTES};
use crate::diagnostics::localized;

const PRIVATE_MODE_BITS: u32 = 0o7777;
const PRIVATE_DIR: u32 = 0o700;
const PRIVATE_FILE: u32 = 0o600;

pub(super) fn ensure_disk(paths: &MarsaPaths, uid: u32, limit: u64) -> io::Result<()> {
    let home = paths.root().parent().ok_or_else(private_disk_error)?;
    let home = home.canonicalize()?;
    let root = home.join(paths.root().file_name().ok_or_else(private_disk_error)?);
    let root_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW_ANY)
        .open(&root)?;
    check_private_dir(&root_file, uid)?;
    let directory = File::from(openat(
        &root_file,
        "vm",
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    check_private_dir(&directory, uid)?;

    // A raw block device must end on a sector boundary.
    let size = limit - limit % SECTOR_BYTES;
    let flags = OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let open = || openat(&directory, c"data.img", flags, Mode::empty());
    let file = match open() {
        Ok(file) => File::from(file),
        Err(nix::errno::Errno::ENOENT) => match create_disk(&directory, flags, size) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => File::from(open()?),
            Err(error) => return Err(error),
        },
        Err(error) => return Err(error.into()),
    };
    disk_metadata(&file, uid)?;
    // Fail promptly if another process holds the disk lock.
    match file.try_lock() {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                localized(
                    "the data disk lock is in use",
                    "قفل قرص البيانات قيد الاستخدام",
                ),
            ));
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    let info = disk_metadata(&file, uid)?;
    if size < info.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            localized(
                "the data disk cannot shrink; choose a limit at least as large as its current capacity",
                "لا يمكن تصغير قرص البيانات؛ اختر حدًا لا يقل عن سعته الحالية",
            ),
        ));
    }
    if size > info.len() {
        file.set_len(size)?;
        file.sync_all()?;
    }
    Ok(())
}

fn disk_metadata(file: &File, uid: u32) -> io::Result<Metadata> {
    let info = file.metadata()?;
    if !is_owned_file(&info, uid)
        || info.mode() & PRIVATE_MODE_BITS != PRIVATE_FILE
        || info.len() == 0
        || !info.len().is_multiple_of(SECTOR_BYTES)
    {
        return Err(private_disk_error());
    }
    Ok(info)
}

fn create_disk(directory: &File, flags: OFlag, size: u64) -> io::Result<()> {
    let nonce = getrandom::u64().map_err(io::Error::other)?;
    let staging = CString::new(format!(".data.img.{nonce:016x}"))?;
    let file = File::from(openat(
        directory,
        staging.as_c_str(),
        flags | OFlag::O_CREAT | OFlag::O_EXCL,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?);
    let publish = || {
        file.set_permissions(Permissions::from_mode(PRIVATE_FILE))?;
        file.set_len(size)?;
        file.sync_all()?;
        publish_disk(directory, &staging)
    };
    if let Err(error) = publish() {
        let _ = unlinkat(directory, staging.as_c_str(), UnlinkatFlags::NoRemoveDir);
        return Err(error);
    }
    directory.sync_all()
}

#[allow(unsafe_code)]
fn publish_disk(directory: &File, staging: &CStr) -> io::Result<()> {
    let fd = directory.as_raw_fd();
    // SAFETY: Both names are valid C strings and the directory descriptor is live.
    // RENAME_EXCL publishes the initialized sparse file without replacing an existing disk.
    let result = unsafe {
        libc::renameatx_np(
            fd,
            staging.as_ptr(),
            fd,
            c"data.img".as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn check_private_dir(directory: &File, uid: u32) -> io::Result<()> {
    let info = directory.metadata()?;
    if info.uid() != uid || info.mode() & PRIVATE_MODE_BITS != PRIVATE_DIR {
        return Err(private_disk_error());
    }
    Ok(())
}

fn private_disk_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        localized(
            "the data disk requires a private directory and a private, user-owned raw image",
            "يتطلب قرص البيانات مجلدًا خاصًا وصورة قرص خام خاصة يملكها المستخدم",
        ),
    )
}
