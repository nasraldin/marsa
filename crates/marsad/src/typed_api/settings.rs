//! Validate a complete replacement and publish its revision after atomic persistence.

use std::{sync::Arc, time::Duration};

pub(super) mod storage;

use marsa_core::settings::{NetworkMode, Settings as Document};
use marsa_proto::host::v1::{
    GetSettingsRequest, SettingsSnapshot, UpdateSettingsRequest, WatchSettingsRequest,
    settings_server::Settings,
};
use marsa_share::{Allowlist, SharePolicy};
use tonic::{Request, Response, Status};

use super::{TypedApi, events::Output};
use storage::{PreparedConfig, WriteError};

#[tonic::async_trait]
impl Settings for TypedApi {
    type WatchStream = Output<SettingsSnapshot>;

    async fn get(
        &self,
        request: Request<GetSettingsRequest>,
    ) -> Result<Response<SettingsSnapshot>, Status> {
        self.authorize(request)?;
        let _admission = self.inner.factory_reset.read()?;
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
        let _admission = self.inner.factory_reset.read()?;
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
        self.apply_settings(expected, None, edit).await
    }

    pub(super) async fn reset_settings(
        &self,
        revision: u64,
        prepared: PreparedConfig,
    ) -> Result<(), Status> {
        self.apply_settings(Some(revision), Some(prepared), |settings, _| {
            *settings = Document::default();
            Ok(())
        })
        .await?;
        Ok(())
    }

    async fn apply_settings<T: Send + 'static>(
        &self,
        expected: Option<u64>,
        prepared: Option<PreparedConfig>,
        edit: impl FnOnce(&mut Document, &SharePolicy) -> Result<T, Status> + Send + 'static,
    ) -> Result<(SettingsSnapshot, T), Status> {
        let _admission = self.inner.factory_reset.read()?;
        let _update = self.inner.edit.lock().await;
        let (prior, snapshot) = self.inner.events.settings();
        if expected.is_some_and(|revision| revision != snapshot.revision) {
            return Err(Status::aborted(
                "settings revision changed; load the latest settings",
            ));
        }
        let next = snapshot
            .revision
            .checked_add(1)
            .ok_or_else(|| Status::resource_exhausted("settings revisions are exhausted"))?;
        let root = self.inner.paths.root().to_owned();
        let uid = self.inner.uid;
        let policy = self.inner.share_policy.clone();
        let proxy = self.proxy.clone();
        let (updated, result) = tokio::task::spawn_blocking(move || {
            let mut settings = prior.clone();
            let result = edit(&mut settings, &policy)?;
            if prior.network.mode != settings.network.mode
                && settings.network.mode != NetworkMode::Userspace
                && proxy.is_some_and(|runtime| runtime.has_approved_proxy())
            {
                return Err(Status::failed_precondition(
                    "disable the approved daemon proxy before changing network mode",
                ));
            }
            if prior == settings && prepared.is_none() {
                return Ok((None, result));
            }
            let text = settings
                .to_toml()
                .map_err(|error| Status::invalid_argument(error.to_string()))?;
            storage::persist(&root, uid, &text, prepared.as_ref()).map_err(
                |error| match error {
                    WriteError::Changed => Status::aborted(error.to_string()),
                    WriteError::Io(error) => {
                        tracing::warn!(kind = ?error.kind(), "could not store settings");
                        Status::internal("could not store settings")
                    }
                },
            )?;
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
