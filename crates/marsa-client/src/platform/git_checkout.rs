//! Publish an already-verified Git checkout to the pinned owner-selected folder.

use std::{
    ffi::OsStr,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use nix::unistd::geteuid;
use rustix::fs::{self, AtFlags, Mode, OFlags, RenameFlags, Stat};
use tempfile::TempDir;

pub(crate) struct GitCheckoutDestination {
    parent_path: PathBuf,
    parent: File,
    parent_identity: Stat,
    stage_path: PathBuf,
    stage_parent: File,
    _stage: File,
    stage_identity: Stat,
    name: String,
}

impl GitCheckoutDestination {
    /// Pin both directories before Git runs and check that publication can be atomic.
    pub(crate) async fn prepare(parent: &Path, staging: &Path) -> io::Result<Self> {
        if !parent.is_absolute() || !staging.is_absolute() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let parent = parent.to_owned();
        let stage_path = staging.to_owned();
        tokio::task::spawn_blocking(move || {
            let parent_path = std::fs::canonicalize(parent)?;
            let (parent, parent_identity) = open_owner_directory(&parent_path, 0o022)?;
            let staging_parent = stage_path.parent().ok_or(io::ErrorKind::InvalidInput)?;
            let (stage_parent, _) = open_owner_directory(staging_parent, 0o022)?;
            let (stage, stage_identity) = open_owner_directory(&stage_path, 0o077)?;
            if stage_identity.st_dev != parent_identity.st_dev {
                return Err(io::ErrorKind::CrossesDevices.into());
            }
            Ok(Self {
                parent_path,
                parent,
                parent_identity,
                stage_path,
                stage_parent,
                _stage: stage,
                stage_identity,
                name: format!("Marsa checkout {}", crate::request_id()?),
            })
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Move the verified tree without replacing an existing directory or file.
    pub(crate) async fn publish(self, staging: TempDir) -> io::Result<PathBuf> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel_on_drop = CancelOnDrop(cancelled.clone());
        tokio::task::spawn_blocking(move || self.rename(staging, &cancelled))
            .await
            .map_err(io::Error::other)?
    }

    fn rename(self, staging: TempDir, cancelled: &AtomicBool) -> io::Result<PathBuf> {
        if staging.path() != self.stage_path {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let stage_name = self
            .stage_path
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?;
        if !same_entry(&self.stage_parent, stage_name, &self.stage_identity) {
            // TempDir normally removes by pathname. That name no longer belongs to us.
            let _ = staging.keep();
            return Err(changed());
        }
        self.check_parent()?;
        // Dropping the awaiting operation wins if the move has not started.
        if cancelled.swap(true, Ordering::AcqRel) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        fs::renameat_with(
            &self.stage_parent,
            stage_name,
            &self.parent,
            self.name.as_str(),
            RenameFlags::NOREPLACE,
        )?;
        if !same_entry(&self.parent, self.name.as_ref(), &self.stage_identity) {
            let _ = staging.keep();
            return Err(changed());
        }
        if let Err(error) = self.check_parent() {
            // Roll back only our own staged directory in the pinned parent.
            if same_entry(&self.parent, self.name.as_ref(), &self.stage_identity) {
                let _ = fs::renameat_with(
                    &self.parent,
                    self.name.as_str(),
                    &self.stage_parent,
                    stage_name,
                    RenameFlags::NOREPLACE,
                );
            }
            if !same_entry(&self.stage_parent, stage_name, &self.stage_identity) {
                let _ = staging.keep();
            }
            return Err(error);
        }
        let _ = staging.keep();
        Ok(self.parent_path.join(self.name))
    }

    fn check_parent(&self) -> io::Result<()> {
        let (_, current) = open_owner_directory(&self.parent_path, 0o022).map_err(|_| changed())?;
        if same_inode(&self.parent_identity, &current) {
            Ok(())
        } else {
            Err(changed())
        }
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn open_owner_directory(path: &Path, disallowed_mode: u32) -> io::Result<(File, Stat)> {
    let file = File::from(fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let stat = fs::fstat(&file)?;
    if stat.st_uid != geteuid().as_raw() || u32::from(stat.st_mode) & disallowed_mode != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok((file, stat))
}

fn same_entry(directory: &File, name: &OsStr, identity: &Stat) -> bool {
    fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|current| same_inode(identity, &current))
}

fn same_inode(a: &Stat, b: &Stat) -> bool {
    (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "the checkout or selected directory moved",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt as _};

    fn staging(parent: &Path) -> TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .unwrap()
    }

    #[tokio::test]
    async fn publishes_the_verified_workspace_to_a_fresh_child() {
        let owner = tempfile::tempdir().unwrap();
        let staging = staging(owner.path());
        let target = tempfile::tempdir().unwrap();
        fs::write(staging.path().join("compose.yaml"), "name: approved").unwrap();
        fs::write(target.path().join("existing"), "unchanged").unwrap();
        let destination = GitCheckoutDestination::prepare(target.path(), staging.path())
            .await
            .unwrap();
        let published = destination.publish(staging).await.unwrap();
        assert_eq!(
            published.parent(),
            Some(target.path().canonicalize().unwrap().as_path())
        );
        assert_eq!(
            fs::read_to_string(published.join("compose.yaml")).unwrap(),
            "name: approved"
        );
        assert_eq!(
            fs::read_to_string(target.path().join("existing")).unwrap(),
            "unchanged"
        );
        assert_eq!(owner.path().read_dir().unwrap().count(), 0);
    }

    #[tokio::test]
    async fn refuses_to_replace_an_empty_checkout_destination() {
        let owner = tempfile::tempdir().unwrap();
        let staging = staging(owner.path());
        let target = tempfile::tempdir().unwrap();
        let destination = GitCheckoutDestination::prepare(target.path(), staging.path())
            .await
            .unwrap();
        let existing = target.path().join(&destination.name);
        fs::create_dir(&existing).unwrap();
        assert_eq!(
            destination.publish(staging).await.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(existing.is_dir());
        assert_eq!(owner.path().read_dir().unwrap().count(), 0);
    }

    #[tokio::test]
    async fn preserves_an_existing_file_at_the_exact_checkout_name() {
        let owner = tempfile::tempdir().unwrap();
        let staging = staging(owner.path());
        let target = tempfile::tempdir().unwrap();
        let destination = GitCheckoutDestination::prepare(target.path(), staging.path())
            .await
            .unwrap();
        let existing = target.path().join(&destination.name);
        fs::write(&existing, b"owner content").unwrap();
        assert_eq!(
            destination.publish(staging).await.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(existing).unwrap(), b"owner content");
        assert_eq!(owner.path().read_dir().unwrap().count(), 0);
    }

    #[tokio::test]
    async fn refuses_a_moved_destination_parent() {
        let owner = tempfile::tempdir().unwrap();
        let staging = staging(owner.path());
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("chosen");
        let moved = root.path().join("moved");
        fs::create_dir(&target).unwrap();
        let destination = GitCheckoutDestination::prepare(&target, staging.path())
            .await
            .unwrap();
        fs::rename(&target, &moved).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(destination.publish(staging).await.is_err());
        assert_eq!(target.read_dir().unwrap().count(), 0);
        assert_eq!(moved.read_dir().unwrap().count(), 0);
    }

    #[tokio::test]
    async fn aborts_queued_publication() {
        let owner = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let staging = staging(owner.path());
        let destination = GitCheckoutDestination::prepare(target.path(), staging.path())
            .await
            .unwrap();
        assert_eq!(
            destination
                .rename(staging, &AtomicBool::new(true))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(target.path().read_dir().unwrap().count(), 0);
    }

    #[tokio::test]
    async fn preserves_a_stage_replacement_without_publishing_it() {
        let owner = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let staging = staging(owner.path());
        let original = staging.path().to_owned();
        let destination = GitCheckoutDestination::prepare(target.path(), &original)
            .await
            .unwrap();
        fs::rename(&original, owner.path().join("moved")).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("replacement"), "unchanged").unwrap();
        assert!(destination.publish(staging).await.is_err());
        assert_eq!(
            fs::read_to_string(original.join("replacement")).unwrap(),
            "unchanged"
        );
        assert_eq!(target.path().read_dir().unwrap().count(), 0);
    }
}
