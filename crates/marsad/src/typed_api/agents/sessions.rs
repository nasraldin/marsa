//! Bind restricted RPCs to principals and project scopes chosen by the trusted owner.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::Instant;
use tonic::Status;

use crate::agent_policy::GrantScope;

use super::{super::requests, Execution, Prepared};

const LIMIT: usize = 64;
const SESSION_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Default)]
pub(super) struct Sessions(Mutex<HashMap<String, Arc<Session>>>);

pub(super) struct Session {
    pub(super) client_id: String,
    pub(super) name: String,
    pub(super) scope: GrantScope,
    pub(super) expires: Instant,
    pub(super) requests: requests::Cache<Prepared, Execution>,
}

impl Sessions {
    pub(super) fn insert(
        &self,
        client_id: String,
        name: String,
        scope: GrantScope,
    ) -> Result<String, Status> {
        let now = Instant::now();
        let mut sessions = self
            .0
            .lock()
            .map_err(|_| Status::internal("agent sessions unavailable"))?;
        sessions.retain(|_, session| session.expires > now);
        if sessions.len() >= LIMIT {
            return Err(Status::resource_exhausted("agent session limit reached"));
        }
        let mut secret = [0; 32];
        getrandom::fill(&mut secret)
            .map_err(|_| Status::internal("cannot create an agent session"))?;
        let token: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();
        if sessions.contains_key(&token) {
            return Err(Status::internal("cannot create an agent session"));
        }
        sessions.insert(
            token.clone(),
            Arc::new(Session {
                client_id,
                name,
                scope,
                expires: now + SESSION_LIFETIME,
                requests: requests::Cache::new(),
            }),
        );
        Ok(token)
    }

    pub(super) fn authenticate(&self, token: &str) -> Option<Arc<Session>> {
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        self.0
            .lock()
            .ok()?
            .get(token)
            .filter(|session| session.expires > Instant::now())
            .cloned()
    }
}
