//! Admit one owner-confirmed factory reset across the private persistent state.

use std::{
    io,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use marsa_core::settings::Settings;
use marsa_proto::host::v1::{
    FactoryResetChallenge, ResetCompleted, ResetResponse, reset_response::Result as ResetResult,
};
use tokio::sync::RwLock;
use tonic::Status;

use crate::{
    factory::{Prepared, ResetError},
    supervisor::LifecyclePermit,
};

use super::{TypedApi, engine::supervisor_status, requests};

const TTL: Duration = Duration::from_secs(60);
type Token = [u8; 32];

pub(super) struct FactoryReset {
    access: RwLock<FactoryGate>,
    pending: Mutex<Option<Challenge>>,
    requests: requests::Cache<Token, ResetResponse>,
}

struct FactoryGate {
    recovery_required: bool,
    _retained: Option<LifecyclePermit>,
}

struct Challenge {
    preparation: String,
    token: Token,
    until: Instant,
    wire_expiry: i64,
    revision: u64,
    prepared: Prepared,
}

impl Challenge {
    fn response(&self) -> ResetResponse {
        ResetResponse {
            result: Some(ResetResult::Challenge(FactoryResetChallenge {
                confirmation_token: self.token.to_vec(),
                expires_at_unix_millis: self.wire_expiry,
            })),
        }
    }
}

impl FactoryReset {
    pub(super) fn new() -> Self {
        Self {
            access: RwLock::new(FactoryGate {
                recovery_required: false,
                _retained: None,
            }),
            pending: Mutex::new(None),
            requests: requests::Cache::new(),
        }
    }

    pub(super) fn read(&self) -> Result<impl Send + '_, Status> {
        let guard = self.access.try_read().map_err(|_| busy())?;
        check_recovery(&guard)?;
        Ok(guard)
    }

    pub(super) async fn close(&self) -> impl Send + '_ {
        self.access.write().await
    }

    fn challenge(&self) -> Result<std::sync::MutexGuard<'_, Option<Challenge>>, Status> {
        self.pending
            .lock()
            .map_err(|_| Status::internal("factory reset confirmation is unavailable"))
    }

    fn take(&self, token: Token, now: Instant) -> Result<Challenge, Status> {
        let mut pending = self.challenge()?;
        if pending
            .as_ref()
            .is_some_and(|challenge| now >= challenge.until)
        {
            *pending = None;
        }
        if pending
            .as_ref()
            .is_some_and(|pending| pending.token == token)
            && let Some(pending) = pending.take()
        {
            return Ok(pending);
        }
        Err(Status::failed_precondition(
            "factory reset confirmation is expired, used or unknown",
        ))
    }
}

impl TypedApi {
    pub(super) async fn prepare_factory_reset(&self, id: String) -> Result<ResetResponse, Status> {
        let _admission = self.inner.factory_reset.read()?;
        let _editor = self.inner.edit.lock().await;
        let revision = self.inner.events.settings_snapshot().revision;
        if let Some(challenge) = self
            .inner
            .factory_reset
            .challenge()?
            .as_ref()
            .filter(|pending| {
                pending.preparation == id
                    && pending.revision == revision
                    && Instant::now() < pending.until
            })
        {
            return Ok(challenge.response());
        }
        let paths = self.inner.paths.clone();
        let uid = self.inner.uid;
        let prepared = tokio::task::spawn_blocking(move || Prepared::capture(&paths, uid))
            .await
            .map_err(|_| Status::internal("factory preparation worker stopped"))?
            .map_err(|_| Status::failed_precondition("could not verify private factory storage"))?;
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|_| Status::internal("could not generate a factory confirmation"))?;
        let wire_expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|now| i64::try_from((now + TTL).as_millis()).ok())
            .ok_or_else(|| Status::internal("could not determine confirmation expiry"))?;
        let challenge = Challenge {
            preparation: id,
            token,
            until: Instant::now() + TTL,
            wire_expiry,
            revision,
            prepared,
        };
        let response = challenge.response();
        *self.inner.factory_reset.challenge()? = Some(challenge);
        Ok(response)
    }

    pub(super) async fn confirm_factory_reset(
        &self,
        id: &str,
        token: Vec<u8>,
    ) -> Result<ResetResponse, Status> {
        let token = token.try_into().map_err(|_| {
            Status::invalid_argument("a factory reset confirmation token is required")
        })?;
        let api = self.clone();
        self.inner
            .factory_reset
            .requests
            .run(
                id,
                token,
                || async move { api.apply_factory_reset(token).await },
            )
            .await
    }

    async fn apply_factory_reset(&self, token: Token) -> Result<ResetResponse, Status> {
        let reset = &self.inner.factory_reset;
        let mut admission = reset.access.try_write().map_err(|_| busy())?;
        check_recovery(&admission)?;
        if *self.inner.closing.borrow() {
            return Err(Status::unavailable("daemon is shutting down"));
        }
        let _editor = self.inner.edit.lock().await;
        let challenge = reset.take(token, Instant::now())?;
        let revision = self.inner.events.settings_snapshot().revision;
        if revision != challenge.revision {
            return Err(Status::aborted(
                "settings changed since factory preparation",
            ));
        }
        let next_revision = revision
            .checked_add(1)
            .ok_or_else(|| Status::resource_exhausted("settings revisions are exhausted"))?;
        let settings = Settings::default();
        let document = settings
            .to_toml()
            .map_err(|_| Status::internal("could not serialize default settings"))?;

        let prepared = tokio::task::spawn_blocking({
            let paths = self.inner.paths.clone();
            let uid = self.inner.uid;
            move || {
                challenge.prepared.validate(&paths, uid)?;
                Ok::<_, io::Error>(challenge.prepared)
            }
        })
        .await
        .map_err(|_| Status::internal("factory verification worker stopped"))?
        .map_err(|_| Status::aborted("private storage changed since factory preparation"))?;

        let permit = if let Some(supervisor) = &self.supervisor {
            let permit = supervisor.reserve(None).await.map_err(supervisor_status)?;
            permit.stop().await.map_err(supervisor_status)?;
            Some(permit)
        } else {
            None
        };

        let inner = self.inner.clone();
        let defaults = settings.clone();
        let encoded = document.clone();
        let result = tokio::task::spawn_blocking(move || {
            inner
                .grants
                .with_lock(|| prepared.apply(&inner.paths, inner.uid, &defaults, &encoded))
                .map_err(|error| ResetError::Refused(io::Error::other(error)))?
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(ResetError::Refused(error))) => {
                tracing::warn!(kind = ?error.kind(), "factory transaction did not commit");
                return Err(Status::aborted("factory reset did not commit"));
            }
            uncertain => {
                if let Ok(Err(ResetError::RecoveryRequired(error))) = uncertain {
                    tracing::error!(kind = ?error.kind(), "private factory recovery is required");
                }
                admission.recovery_required = true;
                admission._retained = permit;
                return Err(recovery_required());
            }
        }
        let idle_applied = if let Some(supervisor) = &self.supervisor {
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
        self.inner
            .events
            .update_settings(settings, document, next_revision, idle_applied);
        self.inner.events.reset_bind_approvals();
        drop(permit);
        Ok(ResetResponse {
            result: Some(ResetResult::Completed(ResetCompleted {})),
        })
    }
}

fn check_recovery(gate: &FactoryGate) -> Result<(), Status> {
    if gate.recovery_required {
        Err(recovery_required())
    } else {
        Ok(())
    }
}

fn busy() -> Status {
    Status::unavailable("factory reset is in progress")
}

fn recovery_required() -> Status {
    Status::unavailable("private factory storage requires daemon recovery")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use marsa_core::paths::MarsaPaths;

    use super::*;

    #[tokio::test]
    async fn factory_reset_invalidates_approved_next_boot_binds() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let home = tempfile::tempdir().unwrap();
        let paths = marsa_core::paths::MarsaPaths::from_root(home.path()).unwrap();
        for directory in [home.path().to_owned(), paths.vm_dir(), paths.state_dir()] {
            if directory != home.path() {
                fs::create_dir(&directory).unwrap();
            }
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let api =
            TypedApi::new(paths, nix::unistd::geteuid().as_raw(), Settings::default()).unwrap();
        let approved = tempfile::tempdir_in("/tmp").unwrap();
        api.inner
            .events
            .stage_once(vec![approved.path().canonicalize().unwrap()]);
        let prepared = api
            .prepare_factory_reset("51000000-0000-4000-8000-000000000001".into())
            .await
            .unwrap();
        let Some(ResetResult::Challenge(challenge)) = prepared.result else {
            panic!("expected a factory reset challenge");
        };
        assert_eq!(api.inner.events.boot_configuration().1.len(), 1);
        let response = api
            .confirm_factory_reset(
                "51000000-0000-4000-8000-000000000002",
                challenge.confirmation_token,
            )
            .await
            .unwrap();
        assert!(matches!(response.result, Some(ResetResult::Completed(_))));
        assert!(api.inner.events.boot_configuration().1.is_empty());
    }

    #[test]
    fn an_expired_factory_confirmation_has_no_live_reservation() {
        let scratch = tempfile::tempdir().unwrap();
        let paths = MarsaPaths::from_root(scratch.path()).unwrap();
        for directory in [paths.root().to_owned(), paths.vm_dir(), paths.state_dir()] {
            fs::create_dir_all(&directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let state = FactoryReset::new();
        let issued = Instant::now();
        let token = [91; 32];
        *state.challenge().unwrap() = Some(Challenge {
            preparation: String::new(),
            token,
            until: issued + TTL,
            wire_expiry: 0,
            revision: 1,
            prepared: Prepared::capture(&paths, nix::unistd::geteuid().as_raw()).unwrap(),
        });
        assert!(state.take([92; 32], issued).is_err());
        assert!(state.challenge().unwrap().is_some());
        assert!(state.take(token, issued + TTL).is_err());
        assert!(state.challenge().unwrap().is_none());
        assert!(state.read().is_ok());
        assert!(!paths.config_file().exists());
        assert!(!paths.data_disk().exists());
        assert!(!paths.grants_file().exists());
    }
}
