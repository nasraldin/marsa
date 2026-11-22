//! Validate a complete replacement and publish its revision after atomic persistence.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};

use marsa_core::settings::Settings as Document;
use marsa_proto::host::v1::{
    GetSettingsRequest, SettingsSnapshot, UpdateSettingsRequest, WatchSettingsRequest,
    settings_server::Settings,
};
use marsa_share::{Allowlist, SharePolicy};
use nix::fcntl::OFlag;
use tonic::{Request, Response, Status};

use super::{TypedApi, events::Output};

#[tonic::async_trait]
impl Settings for TypedApi {
    type WatchStream = Output<SettingsSnapshot>;

    async fn get(
        &self,
        request: Request<GetSettingsRequest>,
    ) -> Result<Response<SettingsSnapshot>, Status> {
        self.authorize(request)?;
        Ok(Response::new(self.inner.events.settings_snapshot()))
    }

    async fn update(
        &self,
        request: Request<UpdateSettingsRequest>,
    ) -> Result<Response<SettingsSnapshot>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let text: Arc<str> = request.settings_toml.into();
        let revision = request.expected_revision;
        let snapshot = self
            .inner
            .settings_requests
            .run(
                &request.request_id,
                (revision, text.clone()),
                || async move { api.update_settings(text, revision).await },
            )
            .await?;
        Ok(Response::new(snapshot))
    }

    async fn watch(
        &self,
        request: Request<WatchSettingsRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        self.authorize(request)?;
        Ok(Response::new(
            self.inner
                .events
                .watch_settings(self.inner.closing.subscribe()),
        ))
    }
}

impl TypedApi {
    async fn update_settings(
        &self,
        text: Arc<str>,
        expected: u64,
    ) -> Result<SettingsSnapshot, Status> {
        let (snapshot, ()) = self
            .edit_settings(Some(expected), move |current, policy| {
                let mut desired = Document::from_toml(&text)
                    .map_err(|error| Status::invalid_argument(error.to_string()))?;
                if desired.shares != current.shares {
                    desired.shares = Allowlist::from_paths(policy.clone(), &desired.shares)
                        .map_err(super::shares::policy_status)?
                        .roots()
                        .to_vec();
                }
                *current = desired;
                Ok(())
            })
            .await?;
        Ok(snapshot)
    }

    pub(super) async fn edit_settings<T: Send + 'static>(
        &self,
        expected: Option<u64>,
        edit: impl FnOnce(&mut Document, &SharePolicy) -> Result<T, Status> + Send + 'static,
    ) -> Result<(SettingsSnapshot, T), Status> {
        let _update = self.inner.edit.lock().await;
        let (prior, once) = self.inner.events.boot_configuration();
        let snapshot = self.inner.events.settings_snapshot();
        if expected.is_some_and(|revision| revision != snapshot.revision) {
            return Err(Status::aborted(
                "settings revision changed; load the latest settings",
            ));
        }
        let next = snapshot
            .revision
            .checked_add(1)
            .ok_or_else(|| Status::resource_exhausted("settings revisions are exhausted"))?;
        let path = self.inner.paths.config_file();
        let uid = self.inner.uid;
        let policy = self.inner.share_policy.clone();
        let (updated, result) = tokio::task::spawn_blocking(move || {
            let mut settings = prior.clone();
            let result = edit(&mut settings, &policy)?;
            if prior == settings {
                return Ok((None, result));
            }
            if settings.shares != prior.shares {
                super::shares::validate_boot_roots(
                    &policy,
                    settings.shares.iter().chain(&once),
                    settings.ui.language,
                )?;
            }
            let text = settings
                .to_toml()
                .map_err(|error| Status::invalid_argument(error.to_string()))?;
            persist(&path, uid, &text).map_err(|error| {
                tracing::warn!(kind = ?error.kind(), "could not store settings");
                Status::internal("could not store settings")
            })?;
            let idle_changed = prior.vm.idle_stop != settings.vm.idle_stop
                || prior.vm.idle_timeout != settings.vm.idle_timeout;
            Ok::<_, Status>((Some((settings, text, idle_changed)), result))
        })
        .await
        .map_err(|_| Status::internal("settings worker stopped"))??;
        let Some((settings, text, idle_changed)) = updated else {
            return Ok((snapshot, result));
        };
        let idle_applied = if idle_changed && let Some(supervisor) = &self.supervisor {
            supervisor
                .set_idle_policy(
                    settings.vm.idle_stop,
                    Duration::from_secs(u64::from(settings.vm.idle_timeout)),
                )
                .await
                .is_ok()
        } else {
            true
        };
        let snapshot = self
            .inner
            .events
            .update_settings(settings, text, next, idle_applied);
        Ok((snapshot, result))
    }
}

fn persist(path: &Path, uid: u32, text: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("settings have no parent directory"))?;
    let directory_fd = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(directory)?;
    if directory_fd.metadata()?.uid() != uid {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.uid() == uid && metadata.nlink() == 1 => (),
        Ok(_) => return Err(io::ErrorKind::PermissionDenied.into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    // The owner-only staging file is created alongside the destination so the
    // replacement is an atomic rename on the same filesystem.
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    // The rename committed in the running daemon even if the directory sync
    // reports a durability issue. Publish the same settings visible on disk.
    if let Err(error) = directory_fd.sync_all() {
        tracing::warn!(kind = ?error.kind(), "settings directory sync failed");
    }
    Ok(())
}
