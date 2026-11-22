//! Anchor artifact paths to open directories and publish with an exclusive rename.

#![cfg_attr(target_os = "macos", allow(unsafe_code))]

use std::{
    ffi::OsStr,
    fs::{self, DirBuilder, File, Permissions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{AtFlags, OFlag, open, openat},
    sys::stat::{Mode, SFlag, fstat, fstatat},
    unistd::{fchdir, geteuid},
};

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

pub(super) fn directory(path: &Path) -> io::Result<File> {
    let start = if path.is_absolute() { "/" } else { "." };
    let mut directory = open(start, DIRECTORY_FLAGS, Mode::empty())?;
    for part in path.components() {
        match part {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, name, DIRECTORY_FLAGS, Mode::empty())?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "artifact directories must not contain parent traversal",
                ));
            }
        }
    }
    Ok(directory.into())
}

pub(super) fn owned_output_parent(directory: &File) -> io::Result<()> {
    let metadata = directory.metadata()?;
    let user = geteuid();
    if user.is_root() || metadata.uid() != user.as_raw() || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the output parent must be owned by the unprivileged user and protected from other writers",
        ));
    }
    Ok(())
}

/// Use the already-open output parent as the base for all temporary paths,
/// including paths passed to curl and tar. Only the synchronous xtask entry
/// point calls this; it never changes cwd while another Rust task runs.
pub(super) fn with_current_directory<T>(
    destination: &File,
    action: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let previous = directory(Path::new("."))?;
    fchdir(destination)?;
    let result = action();
    fchdir(&previous)?;
    result
}

pub(super) struct Scratch(PathBuf);

impl Scratch {
    /// Called inside with_current_directory so even cleanup remains anchored.
    pub(super) fn create() -> io::Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = PathBuf::from(format!(".marsa-release-{suffix}"));
        DirBuilder::new().mode(0o700).create(&path)?;
        let scratch = Self(path);
        // Creation bounds privileges even under a permissive umask. Restore
        // owner access if a restrictive umask removed any of the requested bits.
        fs::set_permissions(scratch.path(), Permissions::from_mode(0o700))?;
        Ok(scratch)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn publish_release(
    source: &File,
    target: &File,
    target_path: &Path,
    target_name: &OsStr,
) -> io::Result<()> {
    let expected = target.metadata()?;
    let current = directory(target_path)
        .and_then(|directory| directory.metadata())
        .map_err(|error| {
            io::Error::other(format!(
                "release output parent changed during staging: {error}"
            ))
        })?;
    if current.dev() != expected.dev() || current.ino() != expected.ino() {
        return Err(io::Error::other(
            "release output parent changed during staging",
        ));
    }
    publish(source, "publish", target, target_name)
}

pub(super) fn require_absent(directory: &File, name: &OsStr) -> io::Result<()> {
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Err(Errno::ENOENT) => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "release resources output already exists",
        )),
    }
}

pub(super) fn require_names(directory: &File, allowed: &[&str]) -> io::Result<()> {
    let mut listing = Dir::openat(directory, ".", DIRECTORY_FLAGS, Mode::empty())?;
    let mut count = 0;
    for entry in listing.iter() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        if !allowed.iter().any(|expected| name == expected.as_bytes()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected guest handoff entry",
            ));
        }
        count += 1;
    }
    if count != allowed.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "guest handoff is missing a required entry",
        ));
    }
    Ok(())
}

pub(super) fn input_file(directory: &File, name: &str) -> io::Result<File> {
    let before = fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if SFlag::from_bits_truncate(before.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err(invalid_input_file(name));
    }
    let flags = OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK;
    let file = File::from(openat(directory, name, flags, Mode::empty())?);
    let after = fstat(&file)?;
    if SFlag::from_bits_truncate(after.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG
        || after.st_ino != before.st_ino
        || after.st_dev != before.st_dev
    {
        return Err(invalid_input_file(name));
    }
    Ok(file)
}

fn invalid_input_file(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("guest handoff entry {name} must be a regular file"),
    )
}

#[cfg(target_os = "macos")]
pub(super) fn publish(
    source: &File,
    source_name: &str,
    target: &File,
    target_name: &OsStr,
) -> io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let source_name = CString::new(source_name)?;
    let target_name = CString::new(target_name.as_encoded_bytes())?;
    // SAFETY: Both descriptors remain open and both names are live C
    // strings. RENAME_EXCL refuses every existing destination entry.
    let result = unsafe {
        nix::libc::renameatx_np(
            source.as_raw_fd(),
            source_name.as_ptr(),
            target.as_raw_fd(),
            target_name.as_ptr(),
            nix::libc::RENAME_EXCL,
        )
    };
    Errno::result(result).map(drop).map_err(Into::into)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(super) fn publish(
    source: &File,
    source_name: &str,
    target: &File,
    target_name: &OsStr,
) -> io::Result<()> {
    use nix::fcntl::{RenameFlags, renameat2};

    renameat2(
        source,
        source_name,
        target,
        target_name,
        RenameFlags::RENAME_NOREPLACE,
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn an_output_created_after_preflight_is_never_replaced() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().canonicalize().unwrap();
        let root = directory(&path).unwrap();
        fs::create_dir(path.join("prepared")).unwrap();
        require_absent(&root, OsStr::new("release")).unwrap();
        let output = path.join("release");
        fs::create_dir(&output).unwrap();
        let existing = fs::metadata(&output).unwrap().ino();

        let error = publish(&root, "prepared", &root, OsStr::new("release")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::metadata(output).unwrap().ino(), existing);
        assert!(path.join("prepared").is_dir());
    }

    #[test]
    fn a_prepared_tree_cannot_publish_when_the_destination_path_changes() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        let destination = root.join("output");
        let moved = root.join("moved");
        let work = destination.join("work");
        fs::create_dir_all(work.join("publish")).unwrap();
        let target = directory(&destination).unwrap();
        let staged = directory(&work).unwrap();
        fs::rename(&destination, &moved).unwrap();
        fs::create_dir(&destination).unwrap();

        let output = OsStr::new("release");
        let error = publish_release(&staged, &target, &destination, output).unwrap_err();

        assert!(error.to_string().contains("release output parent changed"));
        assert!(moved.join("work/publish").is_dir());
        assert!(!moved.join(output).exists());
        assert!(!destination.join(output).exists());
    }
}
