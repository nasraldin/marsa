//! Desktop registry addresses and owner-private Keychain operations.

pub(super) mod helper;
mod probe;
#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use nix::unistd::geteuid;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{docker_socket_path, platform};

use helper::Credential;

const HUB: &str = "docker.io";
const KEY_PREFIX: &str = "https://registries.marsa.invalid/v1/";
const LIMIT: usize = 64;

/// An exact HTTPS Docker registry authority, without URL syntax.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryAddress(String);

impl RegistryAddress {
    /// Docker Hub uses the engine's canonical image namespace.
    pub fn docker_hub() -> Self {
        Self(HUB.to_owned())
    }

    /// Parse a lowercase DNS registry name with an optional explicit port.
    pub fn parse(authority: &str) -> Result<Self, RegistryError> {
        if authority == "index.docker.io" {
            return Ok(Self::docker_hub());
        }
        let (host, valid_port) =
            authority
                .split_once(':')
                .map_or((authority, true), |(host, port)| {
                    (
                        host,
                        !port.starts_with('0')
                            && port.parse::<u16>().is_ok()
                            && port.bytes().all(|byte| byte.is_ascii_digit()),
                    )
                });
        if !valid_port
            || host.len() > 253
            || !host.contains('.')
            || host.ends_with(".localhost")
            || host.ends_with(".invalid")
            || matches!(
                host,
                "registry-1.docker.io" | "auth.docker.io" | "index.docker.io"
            )
            || (host == HUB && authority != HUB)
            || host.parse::<std::net::IpAddr>().is_ok()
            || !host
                .rsplit_once('.')
                .is_some_and(|(_, suffix)| suffix.bytes().any(|byte| byte.is_ascii_lowercase()))
            || !host.split('.').all(valid_dns_label)
        {
            return Err(RegistryError::Address);
        }
        Ok(Self(authority.to_owned()))
    }

    /// The exact lowercase authority used for private registry requests.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this address identifies Docker Hub.
    pub fn is_docker_hub(&self) -> bool {
        self.as_str() == HUB
    }
}

fn valid_dns_label(value: &str) -> bool {
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    value.len() <= 63
        && value.as_bytes().first().is_some_and(alphanumeric)
        && value.as_bytes().last().is_some_and(alphanumeric)
        && value
            .bytes()
            .all(|byte| alphanumeric(&byte) || byte == b'-')
}

/// A user-entered credential. It is intentionally neither printable nor serializable.
pub struct RegistryCredentials {
    username: String,
    secret: String,
}

impl RegistryCredentials {
    /// Validate credentials before they can reach the helper or a registry.
    pub fn new(username: String, secret: String) -> Result<Self, RegistryError> {
        if !valid_username(&username)
            || secret.is_empty()
            || secret.len() > 512
            || secret.chars().any(char::is_control)
        {
            return Err(RegistryError::Credentials);
        }
        Ok(Self { username, secret })
    }
}

fn valid_username(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 256
        && !value.contains(':')
        && !value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    registry: RegistryAddress,
    generation: u64,
}

impl Key {
    fn new(registry: RegistryAddress, generation: u64) -> Self {
        Self {
            registry,
            generation,
        }
    }
}

struct Profile(String);

impl Profile {
    fn new(directory: &Path) -> Self {
        let hash = Sha256::digest(directory.as_os_str().as_encoded_bytes());
        Self(format!(
            "{KEY_PREFIX}{}/",
            URL_SAFE_NO_PAD.encode(&hash[..16])
        ))
    }

    fn url(&self, key: &Key) -> String {
        format!(
            "{}{}/{}",
            self.0,
            URL_SAFE_NO_PAD.encode(key.registry.as_str()),
            key.generation
        )
    }

    fn parse(&self, url: &str) -> Option<Key> {
        let (encoded, generation) = url.strip_prefix(&self.0)?.split_once('/')?;
        if encoded.len() > 350 {
            return None;
        }
        let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
        let registry = RegistryAddress::parse(std::str::from_utf8(&decoded).ok()?).ok()?;
        let generation = generation
            .parse()
            .ok()
            .filter(|generation| *generation > 0)?;
        let key = Key::new(registry, generation);
        (self.url(&key) == url).then_some(key)
    }
}

/// Non-secret metadata about the current desktop credential for a registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    key: Key,
    username: String,
}

impl RegistryEntry {
    /// Exact registry this credential may be used for.
    pub fn address(&self) -> &RegistryAddress {
        &self.key.registry
    }

    /// Saved account name. The credential secret is never returned with the list.
    pub fn username(&self) -> &str {
        &self.username
    }
}

/// Failures for registry credentials or the direct HTTPS connection check.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The registry authority does not have a supported shape.
    #[error("invalid registry hostname")]
    Address,
    /// The username or secret is empty or invalid.
    #[error("invalid registry credentials")]
    Credentials,
    /// The operation requires the logged-in user.
    #[error("registry credentials cannot be used as root")]
    Root,
    /// The verified credential helper is missing, locked, or rejected the operation.
    #[error("the registry Keychain entry is unavailable")]
    Keychain,
    /// The owner state directory could not be locked.
    #[error("could not lock registry settings")]
    State,
    /// Saved credentials changed during the operation.
    #[error("the registry credentials have changed")]
    Changed,
    /// The desktop registry list has reached its bound.
    #[error("the registry credential store is full")]
    Full,
    /// A new credential was saved but an older Keychain entry could not be removed.
    #[error("the credentials were saved, but the old entry is still in Keychain")]
    Cleanup,
    /// The authentication flow cannot keep credentials at the selected authority.
    #[error("the registry's authentication challenge is unsupported")]
    Challenge,
    /// The registry did not approve the credential on a verified HTTPS connection.
    #[error("the registry could not verify the credentials over HTTPS")]
    Connection,
}

/// The desktop registry credentials in a namespace separate from Docker CLI accounts.
pub struct RegistryStore {
    helper: PathBuf,
    directory: PathBuf,
    profile: Profile,
}

impl RegistryStore {
    /// Discover the signed bundled credential helper and owner state.
    pub async fn discover() -> Result<Self, RegistryError> {
        if geteuid().is_root() {
            return Err(RegistryError::Root);
        }
        let helper = platform::bundled_registry_helper()
            .await
            .map_err(|_| RegistryError::Keychain)?;
        let socket = docker_socket_path().map_err(|_| RegistryError::State)?;
        let directory = socket
            .parent()
            .and_then(Path::parent)
            .ok_or(RegistryError::State)?;
        Ok(Self {
            helper,
            directory: directory.to_path_buf(),
            profile: Profile::new(directory),
        })
    }

    /// List the current credential for each saved registry without reading its secret.
    pub async fn list(&self) -> Result<Vec<RegistryEntry>, RegistryError> {
        let mut entries = self.entries().await?;
        entries.dedup_by(|left, right| left.address() == right.address());
        Ok(entries)
    }

    /// Save or replace credentials for the reviewed authority.
    pub async fn save(
        &self,
        address: RegistryAddress,
        credentials: RegistryCredentials,
        expected: Option<RegistryEntry>,
    ) -> Result<(), RegistryError> {
        let _lock = self.lock().await?;
        let entries = self.entries().await?;
        let mut matching = entries.iter().filter(|entry| entry.address() == &address);
        let current = matching.next();
        if current != expected.as_ref() {
            return Err(RegistryError::Changed);
        }
        if entries.len() >= LIMIT && current.is_none() {
            return Err(RegistryError::Full);
        }
        let generation = current
            .map_or(0, |entry| entry.key.generation)
            .checked_add(1)
            .ok_or(RegistryError::Full)?;
        let key = Key::new(address.clone(), generation);
        // The upstream helper deletes any entry at the key before storing. A fresh key
        // preserves the active credential if storing its replacement returns an error.
        helper::store(
            &self.helper,
            &Credential {
                server_url: self.profile.url(&key),
                username: credentials.username,
                secret: credentials.secret,
            },
        )
        .await
        .map_err(|_| RegistryError::Keychain)?;
        for old in current.into_iter().chain(matching) {
            helper::erase(&self.helper, &self.profile.url(&old.key))
                .await
                .map_err(|_| RegistryError::Cleanup)?;
        }
        Ok(())
    }

    /// Remove the reviewed credential after the UI confirms the action.
    pub async fn remove(&self, reviewed: RegistryEntry) -> Result<(), RegistryError> {
        let _lock = self.lock().await?;
        let entries = self.entries().await?;
        let mut matching = entries
            .iter()
            .filter(|entry| entry.address() == reviewed.address());
        if matching.next() != Some(&reviewed) {
            return Err(RegistryError::Changed);
        }
        // Preserve the active record if deleting an older revision fails.
        for entry in matching.rev().chain(std::iter::once(&reviewed)) {
            helper::erase(&self.helper, &self.profile.url(&entry.key))
                .await
                .map_err(|_| RegistryError::Keychain)?;
        }
        Ok(())
    }

    /// Verify the saved credential over HTTPS at the exact registry.
    pub async fn test(&self, reviewed: RegistryEntry) -> Result<(), RegistryError> {
        let credentials = {
            let _lock = self.lock().await?;
            if !self.list().await?.contains(&reviewed) {
                return Err(RegistryError::Changed);
            }
            let value = helper::get(&self.helper, &self.profile.url(&reviewed.key))
                .await
                .map_err(|_| RegistryError::Keychain)?
                .ok_or(RegistryError::Changed)?;
            RegistryCredentials::new(value.username, value.secret)?
        };
        probe::check(reviewed.address(), credentials).await
    }

    async fn entries(&self) -> Result<Vec<RegistryEntry>, RegistryError> {
        let values = helper::list(&self.helper)
            .await
            .map_err(|_| RegistryError::Keychain)?;
        Ok(sorted_keys(values, &self.profile))
    }

    async fn lock(&self) -> Result<std::fs::File, RegistryError> {
        let directory = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            platform::lock_docker_state(&directory, "desktop-registries.lock")
        })
        .await
        .map_err(|_| RegistryError::State)?
        .map_err(|_| RegistryError::State)
    }
}

fn sorted_keys(values: BTreeMap<String, String>, profile: &Profile) -> Vec<RegistryEntry> {
    let mut entries = values
        .into_iter()
        .filter_map(|(url, username)| {
            let key = profile.parse(&url)?;
            valid_username(&username).then_some(RegistryEntry { key, username })
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|one, other| {
        one.address()
            .cmp(other.address())
            .then(other.key.generation.cmp(&one.key.generation))
    });
    entries
}
