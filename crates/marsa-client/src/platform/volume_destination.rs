//! Stage in the protected owner runtime before publishing to a pinned destination.

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use rustix::fs::{self, AtFlags, Mode, OFlags, Stat};

pub(crate) struct VolumeDestination {
    parent_path: PathBuf,
    parent: File,
    name: OsString,
    temporary: StageDirectory,
    original: File,
}

impl VolumeDestination {
    pub(crate) fn stage(destination: &Path, owner_runtime: &Path) -> io::Result<(Self, File)> {
        let name = destination
            .file_name()
            .filter(|_| destination.is_absolute())
            .ok_or(io::ErrorKind::InvalidInput)?
            .to_owned();
        let parent_path = destination
            .parent()
            .ok_or(io::ErrorKind::InvalidInput)?
            .to_owned();
        let parent = open_directory(&parent_path)?;
        // Marsa denies guest shares that contain or overlap the active state path.
        let private = File::from(fs::open(
            owner_runtime,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let identity = fs::fstat(&private)?;
        check_private(&identity)?;
        if identity.st_dev != fs::fstat(&parent)?.st_dev {
            return Err(io::ErrorKind::CrossesDevices.into());
        }
        let temporary = StageDirectory::create(&private)?;
        let original = File::from(fs::openat(
            &temporary.directory,
            STAGED_FILE,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        let staging = Self {
            parent_path,
            parent,
            name,
            temporary,
            original,
        };
        let output = staging.original.try_clone()?;
        Ok((staging, output))
    }

    pub(crate) fn publish(&self, cancelled: &AtomicBool) -> io::Result<()> {
        let original = fs::fstat(&self.original)?;
        self.check_parent()?;
        if !same_entry(&self.temporary.directory, STAGED_FILE.as_ref(), &original) {
            return Err(changed());
        }
        // Cancellation and authorization to begin the atomic publish race once.
        if cancelled.swap(true, Ordering::AcqRel) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        publish(&self.temporary.directory, &self.parent, &self.name)?;
        if !same_entry(&self.parent, &self.name, &original) {
            return Err(changed());
        }
        if let Err(error) = self.check_parent() {
            // Roll back only the staged inode in the pinned directory.
            remove_entry(&self.parent, &self.name, &original);
            return Err(error);
        }
        Ok(())
    }

    fn check_parent(&self) -> io::Result<()> {
        let current = open_directory(&self.parent_path).map_err(|_| changed())?;
        let current = fs::fstat(current)?;
        let pinned = fs::fstat(&self.parent)?;
        if same_inode(&current, &pinned) {
            Ok(())
        } else {
            Err(changed())
        }
    }
}

const STAGED_FILE: &str = "content";

struct StageDirectory {
    parent: File,
    name: String,
    directory: File,
}

impl StageDirectory {
    fn create(parent: &File) -> io::Result<Self> {
        let parent = parent.try_clone()?;
        let name = format!(".marsa-volume-{}.part", crate::request_id()?);
        fs::mkdirat(&parent, name.as_str(), Mode::RWXU)?;
        let identity = fs::statat(&parent, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
        let directory = (|| -> io::Result<File> {
            let directory = File::from(fs::openat(
                &parent,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            let opened = fs::fstat(&directory)?;
            if !same_inode(&identity, &opened) {
                return Err(changed());
            }
            check_private(&opened)?;
            Ok(directory)
        })();
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => {
                remove_directory(&parent, &name, &identity);
                return Err(error);
            }
        };
        Ok(Self {
            parent,
            name,
            directory,
        })
    }
}

impl Drop for StageDirectory {
    fn drop(&mut self) {
        if let Ok(identity) = fs::fstat(&self.directory) {
            remove_directory(&self.parent, &self.name, &identity);
        }
    }
}

impl Drop for VolumeDestination {
    fn drop(&mut self) {
        if let Ok(original) = fs::fstat(&self.original) {
            remove_entry(&self.temporary.directory, STAGED_FILE.as_ref(), &original);
        }
    }
}

fn check_private(directory: &Stat) -> io::Result<()> {
    if directory.st_uid != nix::unistd::geteuid().as_raw() || directory.st_mode & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

fn remove_directory(parent: &File, name: &str, identity: &Stat) {
    if same_entry(parent, name.as_ref(), identity) {
        let _ = fs::unlinkat(parent, name, AtFlags::REMOVEDIR);
    }
}

fn same_entry(directory: &File, name: &OsStr, original: &Stat) -> bool {
    fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|actual| same_inode(&actual, original))
}

fn remove_entry(directory: &File, name: &OsStr, original: &Stat) {
    if same_entry(directory, name, original) {
        let _ = fs::unlinkat(directory, name, AtFlags::empty());
    }
}

fn open_directory(path: &Path) -> io::Result<File> {
    Ok(File::from(fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn publish(stage: &File, directory: &File, destination: &OsStr) -> io::Result<()> {
    Ok(fs::renameat_with(
        stage,
        STAGED_FILE,
        directory,
        destination,
        fs::RenameFlags::NOREPLACE,
    )?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn publish(stage: &File, directory: &File, destination: &OsStr) -> io::Result<()> {
    Ok(fs::linkat(
        stage,
        STAGED_FILE,
        directory,
        destination,
        AtFlags::empty(),
    )?)
}

fn same_inode(a: &Stat, b: &Stat) -> bool {
    (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "the destination or staged file moved",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write as _};

    #[test]
    fn refuses_a_replaced_parent_and_removes_its_staging_from_the_original() {
        let scratch = tempfile::tempdir().unwrap();
        let private = tempfile::Builder::new()
            .permissions(
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
            )
            .tempdir()
            .unwrap();
        let parent = scratch.path().join("parent");
        let moved = scratch.path().join("moved");
        fs::create_dir(&parent).unwrap();
        let destination = parent.join("result");
        let (staging, mut content) =
            VolumeDestination::stage(&destination, private.path()).unwrap();
        content.write_all(b"original stage").unwrap();
        fs::rename(&parent, &moved).unwrap();
        fs::create_dir(&parent).unwrap();
        let substitute = parent.join(&staging.temporary.name);
        fs::write(&substitute, b"replacement").unwrap();
        assert!(staging.publish(&AtomicBool::new(false)).is_err());
        assert!(!destination.exists());
        assert!(!moved.join("result").exists());
        drop(staging);
        assert_eq!(moved.read_dir().unwrap().count(), 0);
        assert_eq!(private.path().read_dir().unwrap().count(), 0);
        assert_eq!(fs::read(substitute).unwrap(), b"replacement");
    }

    #[test]
    fn refuses_replacing_the_staged_inode_or_publishing_after_cancellation() {
        let scratch = tempfile::tempdir().unwrap();
        let destination = scratch.path().join("result");
        let private = tempfile::Builder::new()
            .permissions(
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
            )
            .tempdir()
            .unwrap();
        let (staging, mut content) =
            VolumeDestination::stage(&destination, private.path()).unwrap();
        content.write_all(b"original stage").unwrap();
        assert!(staging.publish(&AtomicBool::new(true)).is_err());
        let substitute = private
            .path()
            .join(&staging.temporary.name)
            .join(STAGED_FILE);
        fs::remove_file(&substitute).unwrap();
        fs::write(&substitute, b"replacement").unwrap();
        assert!(staging.publish(&AtomicBool::new(false)).is_err());
        drop(staging);
        assert!(!destination.exists());
        assert_eq!(fs::read(substitute).unwrap(), b"replacement");
    }
    #[test]
    fn requires_the_private_owner_runtime_before_staging() {
        use std::os::unix::fs::PermissionsExt as _;

        let private = tempfile::Builder::new()
            .permissions(
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
            )
            .tempdir()
            .unwrap();
        let destination = private.path().join("result");
        fs::set_permissions(private.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            VolumeDestination::stage(&destination, private.path()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_eq!(private.path().read_dir().unwrap().count(), 0);
    }
}
