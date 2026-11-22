//! Report sparse host allocation and reclaim unused extents on explicit request.

use std::{
    fs::{File, Metadata, OpenOptions},
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

use crate::{guest_control::GuestControlError, supervisor::State};

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
        let request = self.authorize(request)?;
        let api = self.clone();
        self.inner
            .reclaim_requests
            .run(&request.request_id, (), || async move {
                api.reclaim_from_running_guest().await
            })
            .await
            .map(Response::new)
    }

    async fn prune_preview(
        &self,
        request: Request<PrunePreviewRequest>,
    ) -> Result<Response<PrunePreviewResponse>, Status> {
        self.authorize(request)?;
        Err(Status::unimplemented("prune preview is unavailable"))
    }
}

impl TypedApi {
    async fn reclaim_from_running_guest(&self) -> Result<DiskReclaimResponse, Status> {
        let _admission = self.inner.factory_reset.read()?;
        let _active = self
            .inner
            .reclaim_active
            .try_lock()
            .map_err(|_| Status::resource_exhausted("disk reclaim is already running"))?;
        let supervisor = self
            .supervisor
            .as_ref()
            .filter(|supervisor| matches!(supervisor.snapshot().state, State::Ready | State::Idle))
            .ok_or_else(|| Status::failed_precondition("disk reclaim requires a ready VM"))?;
        let control = self.guest_control.as_ref().ok_or_else(|| {
            Status::failed_precondition("there is no running guest control session")
        })?;
        let session = control.trim_session().map_err(trim_error)?;
        let before = self.reclaim_disk().await?;

        if *self.inner.closing.borrow()
            || !session.is_current(control)
            || !matches!(supervisor.snapshot().state, State::Ready | State::Idle)
        {
            return Err(Status::unavailable("guest session changed before reclaim"));
        }
        let result = session.run().await.map_err(trim_error)?;
        let after = self.reclaim_disk().await?;
        if !session.is_current(control)
            || !matches!(supervisor.snapshot().state, State::Ready | State::Idle)
        {
            return Err(Status::unavailable("guest session changed during reclaim"));
        }
        if before.identity() != after.identity() || before.metadata.len() != after.metadata.len() {
            return Err(Status::failed_precondition(
                "data disk changed during reclaim",
            ));
        }
        let mut usage = after.usage();
        if result.bytes_trimmed > usage.host_capacity_bytes
            || result.data_disk_used_bytes > usage.host_capacity_bytes
        {
            return Err(Status::data_loss(
                "guest trim counters exceed the data disk capacity",
            ));
        }
        if let Some(sample) = self.inner.events.disk_sample()
            && result.data_disk_used_bytes <= sample.disk_total_bytes
        {
            usage.guest_used_bytes = result.data_disk_used_bytes;
            usage.guest_capacity_bytes = sample.disk_total_bytes;
            usage.guest_usage_available = true;
        }
        Ok(DiskReclaimResponse {
            guest_trimmed_bytes: result.bytes_trimmed,
            host_reclaimed_bytes: before
                .usage()
                .host_allocated_bytes
                .saturating_sub(usage.host_allocated_bytes),
            usage: Some(usage),
        })
    }

    async fn reclaim_disk(&self) -> Result<HostDisk, Status> {
        let root = self.inner.paths.root().to_owned();
        let uid = self.inner.uid;
        tokio::task::spawn_blocking(move || open_host_disk(&root, uid))
            .await
            .map_err(|_| Status::internal("disk usage worker stopped"))?
            .map_err(|_| Status::failed_precondition("could not inspect the private data disk"))?
            .ok_or_else(|| Status::failed_precondition("the private data disk is unavailable"))
    }
}

fn trim_error(error: GuestControlError) -> Status {
    match error {
        GuestControlError::Busy => Status::resource_exhausted("guest control is busy"),
        GuestControlError::TimedOut(_) => Status::deadline_exceeded("guest trim timed out"),
        GuestControlError::Disconnected => Status::unavailable("guest control disconnected"),
        GuestControlError::TrimRejected(_) => {
            Status::failed_precondition("guest could not trim the data disk")
        }
        _ => Status::internal("guest trim failed"),
    }
}

struct HostDisk {
    // Keep the inode alive while the trim runs, so a replacement cannot
    // recycle it before we compare the two allocation observations.
    _file: File,
    metadata: Metadata,
}

impl HostDisk {
    fn identity(&self) -> (u64, u64) {
        (self.metadata.dev(), self.metadata.ino())
    }

    fn usage(&self) -> DiskUsageResponse {
        DiskUsageResponse {
            host_allocated_bytes: self.metadata.blocks().saturating_mul(512),
            host_capacity_bytes: self.metadata.len(),
            ..Default::default()
        }
    }
}

fn host_usage(root: &Path, uid: u32) -> io::Result<DiskUsageResponse> {
    Ok(open_host_disk(root, uid)?
        .map(|disk| disk.usage())
        .unwrap_or_default())
}

fn open_host_disk(root: &Path, uid: u32) -> io::Result<Option<HostDisk>> {
    let directories = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let files = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let root = match OpenOptions::new()
        .read(true)
        .custom_flags(directories.bits())
        .open(root)
    {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    check_directory(&root, uid)?;
    let directory = match openat(&root, "vm", directories, Mode::empty()) {
        Ok(directory) => File::from(directory),
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    check_directory(&directory, uid)?;
    let file = match openat(&directory, "data.img", files, Mode::empty()) {
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
    Ok(Some(HostDisk {
        _file: file,
        metadata,
    }))
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

    #[tokio::test]
    async fn concurrent_reclaim_is_rejected_while_one_is_active() {
        let home = tempfile::tempdir().unwrap();
        let api = TypedApi::new(
            marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap(),
            nix::unistd::geteuid().as_raw(),
            Default::default(),
        )
        .unwrap();
        let _active = api.inner.reclaim_active.lock().await;
        let error = api.reclaim_from_running_guest().await.unwrap_err();
        assert_eq!(error.code(), tonic::Code::ResourceExhausted);
    }

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
