//! Persist only expiring ordinary operate grants in a versioned private document.

use std::{
    collections::HashSet,
    io,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_core::{paths::MarsaPaths, settings::AgentSettings};
use marsa_proto::host::v1::{AgentGrant, AgentOperation, PermissionTier};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Decision, Effect, decide, platform, tier};

const VERSION: u32 = 1;
const MAX_GRANTS: usize = 1024;
const MAX_BYTES: usize = 512 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Error)]
/// Reasons the daemon cannot read or apply a grant.
pub enum GrantError {
    /// Missing or malformed authenticated client or project scope.
    #[error("invalid agent project scope")]
    InvalidScope,
    /// A stored record is malformed, duplicated, incompatible or too large.
    #[error("invalid agent grants document")]
    InvalidDocument,
    /// The operation cannot be covered by a persistent grant.
    #[error("the operation is ineligible for a project grant")]
    Ineligible,
    /// The bounded grants list is full.
    #[error("the agent grants list is full")]
    Full,
    /// The host clock could not produce a valid timestamp.
    #[error("invalid host time for agent grants")]
    Clock,
    /// The filesystem or host entropy source could not complete the operation.
    #[error("could not access private agent grants")]
    Io(#[from] io::Error),
}

/// Stable identity of an authenticated client and the resolved host project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantScope {
    client_id: String,
    project: String,
    device: u64,
    inode: u64,
}

impl GrantScope {
    /// Resolve the project using a client ID obtained from the authenticated session.
    ///
    /// Run this filesystem operation off the async runtime.
    pub fn resolve(client_id: &str, project: &Path) -> Result<Self, GrantError> {
        if !valid_id(client_id) || !project.to_str().is_some_and(valid_path) {
            return Err(GrantError::InvalidScope);
        }
        let canonical = project.canonicalize()?;
        let metadata = canonical.metadata()?;
        let project = canonical.to_str().filter(|path| valid_path(path));
        let Some(project) = project.filter(|_| metadata.is_dir()) else {
            return Err(GrantError::InvalidScope);
        };
        Ok(Self {
            client_id: client_id.into(),
            project: project.into(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub(crate) fn project(&self) -> &str {
        &self.project
    }

    fn validate(&self) -> bool {
        valid_id(&self.client_id) && valid_path(&self.project)
    }
}

/// A project grant for one ordinary operate-tier action.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    id: String,
    scope: GrantScope,
    operation: i32,
    last_used_ms: i64,
}

impl Grant {
    /// Convert an active grant into the typed public metadata.
    pub fn to_wire(&self) -> AgentGrant {
        AgentGrant {
            id: self.id.clone(),
            client_id: self.scope.client_id.clone(),
            project_path: self.scope.project.clone(),
            operation: self.operation,
            expires_at_unix_millis: self.last_used_ms + LIFETIME_MS,
        }
    }

    fn active(&self, now: i64) -> bool {
        (self.last_used_ms..self.last_used_ms + LIFETIME_MS).contains(&now)
    }

    fn valid(&self) -> bool {
        self.id.len() == 32
            && self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && self.scope.validate()
            && AgentOperation::try_from(self.operation).ok().and_then(tier)
                == Some(PermissionTier::Operate)
            && (0..=i64::MAX - LIFETIME_MS).contains(&self.last_used_ms)
    }
}

/// A validated set of persisted project grants.
#[derive(Debug, Default)]
pub struct Grants(Vec<Grant>);

impl Grants {
    /// Iterate over active grants at the supplied host time.
    pub fn active(&self, now: SystemTime) -> Result<impl Iterator<Item = &Grant>, GrantError> {
        let now = timestamp(now)?;
        Ok(self.0.iter().filter(move |grant| grant.active(now)))
    }

    /// Evaluate a request and renew its grant only when the grant authorizes it.
    ///
    /// Call inside a store transaction to persist grant activity before returning.
    pub fn evaluate(
        &mut self,
        scope: &GrantScope,
        operation: AgentOperation,
        effect: Effect,
        settings: &AgentSettings,
        now: SystemTime,
    ) -> Result<Decision, GrantError> {
        let now = timestamp(now)?;
        let decision = decide(operation, effect, settings);
        if let Decision::Approve {
            can_grant_for_project: true,
            ..
        } = decision
            && let Some(grant) = self.0.iter_mut().find(|grant| {
                grant.scope == *scope && grant.operation == operation as i32 && grant.active(now)
            })
        {
            grant.last_used_ms = now;
            return Ok(Decision::Allow);
        }
        Ok(decision)
    }

    /// Record an explicit human approval for a grantable ordinary operation.
    pub fn record_approval(
        &mut self,
        scope: GrantScope,
        operation: AgentOperation,
        effect: Effect,
        settings: &AgentSettings,
        now: SystemTime,
    ) -> Result<AgentGrant, GrantError> {
        if !scope.validate() {
            return Err(GrantError::InvalidScope);
        }
        if !matches!(
            decide(operation, effect, settings),
            Decision::Approve {
                can_grant_for_project: true,
                ..
            }
        ) {
            return Err(GrantError::Ineligible);
        }
        let now = timestamp(now)?;
        self.0.retain(|grant| grant.active(now));
        if let Some(grant) = self
            .0
            .iter_mut()
            .find(|grant| grant.scope == scope && grant.operation == operation as i32)
        {
            grant.last_used_ms = now;
            return Ok(grant.to_wire());
        }
        if self.0.len() == MAX_GRANTS {
            return Err(GrantError::Full);
        }
        let mut id = [0_u8; 16];
        getrandom::fill(&mut id).map_err(io::Error::other)?;
        let grant = Grant {
            id: format!("{:032x}", u128::from_be_bytes(id)),
            scope,
            operation: operation as i32,
            last_used_ms: now,
        };
        let wire = grant.to_wire();
        self.0.push(grant);
        Ok(wire)
    }

    /// Revoke a project grant by its daemon-issued ID.
    pub fn revoke(&mut self, id: &str) -> bool {
        let prior = self.0.len();
        self.0.retain(|grant| grant.id != id);
        self.0.len() != prior
    }
}

/// The daemon's single synchronized handle to its private project grants file.
pub struct GrantStore {
    paths: MarsaPaths,
    uid: u32,
    lock: Mutex<()>,
}

impl GrantStore {
    /// Prepare the store for the daemon's existing private state directory.
    pub fn new(paths: MarsaPaths, uid: u32) -> Self {
        Self {
            paths,
            uid,
            lock: Mutex::new(()),
        }
    }

    /// Read the bounded store. Run off the async runtime.
    pub fn load(&self) -> Result<Grants, GrantError> {
        let _lock = self.lock.lock().map_err(|_| GrantError::InvalidDocument)?;
        self.read().map(|(grants, _)| grants)
    }

    /// Apply an atomic update, publishing no changes if the closure fails.
    ///
    /// The daemon should perform this operation off the async runtime and share
    /// one store handle between all services.
    pub fn update<T>(
        &self,
        edit: impl FnOnce(&mut Grants) -> Result<T, GrantError>,
    ) -> Result<T, GrantError> {
        let _lock = self.lock.lock().map_err(|_| GrantError::InvalidDocument)?;
        let (mut grants, before) = self.read()?;
        let result = edit(&mut grants)?;
        let bytes = encode(&grants)?;
        if before.as_deref() != Some(bytes.as_slice()) && !(before.is_none() && grants.0.is_empty())
        {
            platform::write(&self.paths, self.uid, &bytes)?;
        }
        Ok(result)
    }

    pub(crate) fn with_lock<T>(&self, operation: impl FnOnce() -> T) -> Result<T, GrantError> {
        let _lock = self.lock.lock().map_err(|_| GrantError::InvalidDocument)?;
        Ok(operation())
    }

    fn read(&self) -> Result<(Grants, Option<Vec<u8>>), GrantError> {
        let bytes = platform::read(&self.paths, self.uid, MAX_BYTES + 1)?;
        let grants = bytes
            .as_deref()
            .map(decode)
            .transpose()?
            .unwrap_or_default();
        Ok((grants, bytes))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document<T> {
    version: u32,
    grants: T,
}

fn encode(grants: &Grants) -> Result<Vec<u8>, GrantError> {
    let bytes = serde_json::to_vec(&Document {
        version: VERSION,
        grants: &grants.0,
    })
    .map_err(|_| GrantError::InvalidDocument)?;
    if bytes.len() > MAX_BYTES {
        return Err(GrantError::Full);
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<Grants, GrantError> {
    if bytes.len() > MAX_BYTES {
        return Err(GrantError::InvalidDocument);
    }
    let document: Document<Vec<Grant>> =
        serde_json::from_slice(bytes).map_err(|_| GrantError::InvalidDocument)?;
    let mut keys = HashSet::new();
    let mut ids = HashSet::new();
    if document.version != VERSION
        || document.grants.len() > MAX_GRANTS
        || document.grants.iter().any(|grant| {
            !grant.valid()
                || !keys.insert((&grant.scope, grant.operation))
                || !ids.insert(&grant.id)
        })
    {
        return Err(GrantError::InvalidDocument);
    }
    Ok(Grants(document.grants))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn valid_path(value: &str) -> bool {
    let path = PathBuf::from(value);
    path.is_absolute()
        && path.parent().is_some()
        && value.len() <= MAX_PATH_BYTES
        && !value.chars().any(char::is_control)
        && !path.components().any(|part| part == Component::ParentDir)
}

fn timestamp(now: SystemTime) -> Result<i64, GrantError> {
    let elapsed = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GrantError::Clock)?;
    i64::try_from(elapsed.as_millis())
        .ok()
        .filter(|time| *time <= i64::MAX - LIFETIME_MS)
        .ok_or(GrantError::Clock)
}
