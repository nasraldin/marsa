//! Report actual sparse host allocation and the current guest filesystem sample.

use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use marsa_proto::host::v1::{
    DiskReclaimRequest, DiskReclaimResponse, DiskUsageRequest, DiskUsageResponse,
    PrunePreviewRequest, PrunePreviewResponse, disk_server::Disk,
};
use nix::{
    errno::Errno,
    fcntl::{OFlag, openat},
    sys::stat::Mode,
};
use tonic::{Request, Response, Status};

use super::TypedApi;

#[tonic::async_trait]
impl Disk for TypedApi {
    async fn usage(
        &self,
        request: Request<DiskUsageRequest>,
    ) -> Result<Response<DiskUsageResponse>, Status> {
        self.authorize(request)?;
        let _admission = self.inner.factory_reset.read()?;
        let path = self.inner.paths.root().to_owned();
        let uid = self.inner.uid;
        let mut usage = tokio::task::spawn_blocking(move || host_usage(&path, uid))
            .await
            .map_err(|_| Status::internal("disk usage worker stopped"))?
            .map_err(|error| {
                tracing::warn!(kind = ?error.kind(), "could not inspect private disk");
                Status::failed_precondition("could not inspect the private data disk")
            })?;
        if let Some(sample) = self.inner.events.disk_sample() {
            usage.guest_capacity_bytes = sample.disk_total_bytes;
            usage.guest_used_bytes = sample.disk_used_bytes;
            usage.guest_usage_available = true;
        }
        Ok(Response::new(usage))
    }

    async fn reclaim(
        &self,
        request: Request<DiskReclaimRequest>,
    ) -> Result<Response<DiskReclaimResponse>, Status> {
        self.authorize(request)?;
        Err(Status::unimplemented("disk reclamation is unavailable"))
    }

    async fn prune_preview(
        &self,
        request: Request<PrunePreviewRequest>,
    ) -> Result<Response<PrunePreviewResponse>, Status> {
        self.authorize(request)?;
        Err(Status::unimplemented("prune preview is unavailable"))
    }
}

fn host_usage(root: &Path, uid: u32) -> io::Result<DiskUsageResponse> {
    let directories = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let files = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let root = match OpenOptions::new()
        .read(true)
        .custom_flags(directories.bits())
        .open(root)
    {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(DiskUsageResponse::default());
        }
        Err(error) => return Err(error),
    };
    check_directory(&root, uid)?;
    let directory = match openat(&root, "vm", directories, Mode::empty()) {
        Ok(directory) => File::from(directory),
        Err(Errno::ENOENT) => return Ok(DiskUsageResponse::default()),
        Err(error) => return Err(error.into()),
    };
    check_directory(&directory, uid)?;
    let file = match openat(&directory, "data.img", files, Mode::empty()) {
        Ok(file) => File::from(file),
        Err(Errno::ENOENT) => return Ok(DiskUsageResponse::default()),
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
    Ok(DiskUsageResponse {
        host_allocated_bytes: metadata.blocks().saturating_mul(512),
        host_capacity_bytes: metadata.len(),
        ..Default::default()
    })
}

fn check_directory(directory: &File, uid: u32) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    use super::*;

    #[test]
    fn reports_actual_sparse_capacity_and_rejects_redirects() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let uid = nix::unistd::geteuid().as_raw();
        assert_eq!(
            host_usage(root.path(), uid).unwrap(),
            DiskUsageResponse::default()
        );
        let directory = root.path().join("vm");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            host_usage(root.path(), uid).unwrap(),
            DiskUsageResponse::default()
        );
        let path = directory.join("data.img");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.set_len(1 << 30).unwrap();
        let usage = host_usage(root.path(), uid).unwrap();
        assert_eq!(usage.host_capacity_bytes, 1 << 30);
        assert_eq!(
            usage.host_allocated_bytes,
            file.metadata().unwrap().blocks() * 512
        );
        assert!(!usage.guest_usage_available);

        let redirected = directory.join("other.img");
        fs::rename(&path, &redirected).unwrap();
        symlink(&redirected, &path).unwrap();
        assert!(host_usage(root.path(), uid).is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&redirected, &path).unwrap();
        assert!(host_usage(root.path(), uid).is_err());
    }
}
