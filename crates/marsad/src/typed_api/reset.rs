//! Prepare and consume owner-only settings reset confirmations.

use std::{
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use marsa_proto::host::v1::{
    ResetCompleted, ResetRequest, ResetResponse, SettingsResetChallenge, reset_request::Action,
    reset_response::Result as ResetResult,
};
use tonic::Status;

use super::{
    TypedApi, requests,
    settings::storage::{self, PreparedConfig},
};

const TTL: Duration = Duration::from_secs(60);
const TOKEN_BYTES: usize = 32;
type Token = [u8; TOKEN_BYTES];

pub(super) struct SettingsReset {
    challenge: Mutex<Option<Challenge>>,
    requests: requests::Cache<Token, ResetResponse>,
}

struct Challenge {
    preparation: String,
    token: Token,
    revision: u64,
    prepared: PreparedConfig,
    expires: Instant,
    expires_at_unix_millis: i64,
}

impl Challenge {
    fn response(&self) -> ResetResponse {
        ResetResponse {
            result: Some(ResetResult::SettingsChallenge(SettingsResetChallenge {
                confirmation_token: self.token.to_vec(),
                expires_at_unix_millis: self.expires_at_unix_millis,
            })),
        }
    }
}

impl SettingsReset {
    pub(super) fn new() -> Self {
        Self {
            challenge: Mutex::new(None),
            requests: requests::Cache::new(),
        }
    }

    fn take(&self, token: &Token, now: Instant) -> Result<Challenge, Status> {
        let mut challenge = self
            .challenge
            .lock()
            .map_err(|_| Status::internal("reset confirmation state is unavailable"))?;
        if challenge
            .as_ref()
            .is_some_and(|pending| now >= pending.expires)
        {
            *challenge = None;
        }
        if challenge
            .as_ref()
            .is_some_and(|pending| &pending.token == token)
            && let Some(pending) = challenge.take()
        {
            return Ok(pending);
        }
        Err(Status::failed_precondition(
            "settings reset confirmation is expired, used or unknown",
        ))
    }
}

impl TypedApi {
    pub(super) async fn engine_reset(
        &self,
        request: ResetRequest,
    ) -> Result<ResetResponse, Status> {
        let id = requests::request_id(&request.request_id)?;
        match request.action {
            Some(Action::PrepareSettings(_)) => self.prepare_settings_reset(id).await,
            Some(Action::Settings(confirmation)) => {
                let token = confirmation.confirmation_token.try_into().map_err(|_| {
                    Status::invalid_argument("a settings reset confirmation token is required")
                })?;
                let api = self.clone();
                self.inner
                    .settings_reset
                    .requests
                    .run(&id, token, || async move {
                        let pending = api.inner.settings_reset.take(&token, Instant::now())?;
                        api.reset_settings(pending.revision, pending.prepared)
                            .await?;
                        Ok(ResetResponse {
                            result: Some(ResetResult::Completed(ResetCompleted {})),
                        })
                    })
                    .await
            }
            Some(Action::PrepareFactory(_) | Action::ConfirmFactory(_)) => {
                Err(Status::unimplemented("factory reset is not available"))
            }
            None => Err(Status::invalid_argument("a reset action is required")),
        }
    }

    async fn prepare_settings_reset(&self, id: String) -> Result<ResetResponse, Status> {
        let _edit = self.inner.edit.lock().await;
        let revision = self.inner.events.settings_snapshot().revision;
        let state = &self.inner.settings_reset.challenge;
        if let Some(challenge) = state
            .lock()
            .map_err(|_| Status::internal("reset confirmation state is unavailable"))?
            .as_ref()
            .filter(|pending| {
                pending.preparation == id
                    && pending.revision == revision
                    && Instant::now() < pending.expires
            })
        {
            return Ok(challenge.response());
        }
        let root = self.inner.paths.root().to_owned();
        let uid = self.inner.uid;
        let prepared = tokio::task::spawn_blocking(move || storage::prepare(&root, uid))
            .await
            .map_err(|_| Status::internal("reset preparation worker stopped"))?
            .map_err(|_| {
                Status::failed_precondition("could not verify the private configuration")
            })?;

        let mut token = [0; TOKEN_BYTES];
        getrandom::fill(&mut token)
            .map_err(|_| Status::internal("could not generate a reset confirmation"))?;
        let expires = Instant::now() + TTL;
        let expires_at_unix_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from((elapsed + TTL).as_millis()).ok())
            .ok_or_else(|| Status::internal("could not determine reset confirmation expiry"))?;
        let challenge = Challenge {
            preparation: id,
            token,
            revision,
            prepared,
            expires,
            expires_at_unix_millis,
        };
        let response = challenge.response();
        *state
            .lock()
            .map_err(|_| Status::internal("reset confirmation state is unavailable"))? =
            Some(challenge);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;

    #[test]
    fn an_expired_confirmation_cannot_be_consumed() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let reset = SettingsReset::new();
        let issued = Instant::now();
        let token = [1; TOKEN_BYTES];
        *reset.challenge.lock().unwrap() = Some(Challenge {
            preparation: String::new(),
            token,
            revision: 1,
            prepared: storage::prepare(root.path(), nix::unistd::geteuid().as_raw()).unwrap(),
            expires: issued + TTL,
            expires_at_unix_millis: 0,
        });

        assert!(reset.take(&[2; TOKEN_BYTES], issued).is_err());
        assert!(reset.challenge.lock().unwrap().is_some());
        assert!(reset.take(&token, issued + TTL).is_err());
        assert!(reset.challenge.lock().unwrap().is_none());
    }
}
