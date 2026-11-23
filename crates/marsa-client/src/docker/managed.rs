//! Configure and persist desktop-managed Docker engines using the external transport checks.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;

use crate::{docker_socket_path, platform, request_id};

use super::{
    DockerError, ExternalEndpoint,
    contexts::{SshEndpoint, TlsEndpoint},
    inventory::ExternalTransport,
};

const STORE_FILE: &str = "desktop-endpoints.json";
const STORE_LIMIT: usize = 1024 * 1024;
const MAX_ENDPOINTS: usize = 64;
const ADDRESS_LIMIT: usize = 2048;
const DISPLAY_LIMIT: usize = 256;
const MANAGED_PREFIX: &str = "managed:";

/// A persistently identified Docker endpoint configured in Marsa.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedEndpoint {
    /// Stable Marsa source ID, independent of the name and transport.
    pub id: String,
    /// The last saved endpoint settings.
    pub config: ManagedEndpointConfig,
}

/// Editable settings for a Docker endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedEndpointConfig {
    /// A display label chosen by the user.
    pub name: String,
    /// The required transport and its connection settings.
    pub transport: ManagedEndpointTransport,
}

/// Transports supported for manual Docker connections.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedEndpointTransport {
    /// Authenticate a local socket and its peer against the current user.
    Unix {
        /// Absolute socket path, resolved and pinned before connecting.
        socket: PathBuf,
    },
    /// Use OpenSSH and the user's existing identity and known hosts.
    Ssh {
        /// Docker SSH URI, optionally including user, port, and remote socket.
        address: String,
    },
    /// Verify the server with the supplied CA and authenticate with a client certificate.
    Tls {
        /// TLS address as `tcp://host[:port]` or `https://host[:port]`.
        address: String,
        /// Absolute path to the trusted CA certificate file.
        ca: PathBuf,
        /// Absolute path to the client certificate file.
        cert: PathBuf,
        /// Absolute path to the owner-private client key file.
        key: PathBuf,
    },
}

/// Engine identity returned over an authenticated Docker connection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VerifiedEngine {
    /// Identifier reported by Docker Engine.
    #[serde(rename = "ID")]
    pub id: String,
    /// Hostname reported by Docker Engine.
    #[serde(rename = "Name")]
    pub name: String,
    /// Version reported by Docker Engine.
    #[serde(rename = "ServerVersion")]
    pub version: String,
}

impl VerifiedEngine {
    pub(super) fn valid(&self) -> bool {
        [&self.id, &self.name, &self.version]
            .into_iter()
            .all(|field| valid_text(field, DISPLAY_LIMIT))
    }
}

/// Input, authentication, or persistence failure for a managed endpoint.
#[derive(Debug, Error)]
pub enum ManagedEndpointError {
    /// The display name is empty, too long, or contains unsafe characters.
    #[error("invalid Docker endpoint name")]
    InvalidName,
    /// The endpoint address is malformed for its selected transport.
    #[error("invalid Docker endpoint address")]
    InvalidAddress,
    /// A TLS credential path is invalid.
    #[error("expected absolute paths to Docker TLS credential files")]
    InvalidCredentialPath,
    /// The Unix socket belongs to Marsa's built-in environment.
    #[error("Marsa's own Docker socket is already configured")]
    LocalMarsa,
    /// Docker endpoints are unavailable to root.
    #[error("managed Docker endpoints cannot be used as root")]
    Root,
    /// The managed endpoint has already been removed or does not exist.
    #[error("managed Docker endpoint does not exist")]
    NotFound,
    /// Saved connection settings could not be read or changed.
    #[error("could not read or write the managed Docker endpoint store")]
    Store(#[source] io::Error),
    /// Mutual TLS credentials could not be read or validated.
    #[error("invalid or inaccessible Docker TLS credentials")]
    Credentials(#[source] io::Error),
    /// The engine connection could not be authenticated or did not answer.
    #[error("managed Docker connection test failed")]
    Connection(#[from] DockerError),
    /// The engine did not return a valid identity.
    #[error("Docker returned an invalid engine identity")]
    InvalidEngineIdentity,
}

type Result<T> = std::result::Result<T, ManagedEndpointError>;

impl ManagedEndpointConfig {
    /// Check configuration fields without connecting to the engine.
    pub fn validate(&self) -> Result<()> {
        if !valid_text(&self.name, DISPLAY_LIMIT) {
            return Err(ManagedEndpointError::InvalidName);
        }
        match &self.transport {
            ManagedEndpointTransport::Unix { socket } if valid_path(socket) => Ok(()),
            ManagedEndpointTransport::Ssh { address }
                if valid_text(address, ADDRESS_LIMIT)
                    && address
                        .strip_prefix("ssh://")
                        .and_then(SshEndpoint::parse)
                        .is_some() =>
            {
                Ok(())
            }
            ManagedEndpointTransport::Tls {
                address,
                ca,
                cert,
                key,
            } if valid_text(address, ADDRESS_LIMIT)
                && tls_address(address).is_some_and(TlsEndpoint::valid_address) =>
            {
                if [ca, cert, key].into_iter().all(|path| valid_path(path)) {
                    Ok(())
                } else {
                    Err(ManagedEndpointError::InvalidCredentialPath)
                }
            }
            _ => Err(ManagedEndpointError::InvalidAddress),
        }
    }

    /// Authenticate a draft and read Docker's identity without saving the settings.
    pub async fn test(&self) -> Result<VerifiedEngine> {
        check_user()?;
        let mut endpoint = ExternalEndpoint::managed(
            format!("{MANAGED_PREFIX}draft"),
            self.capture_transport().await?,
        );
        endpoint.pin_ssh().await;
        endpoint.verify_engine().await
    }

    pub(super) async fn capture_transport(&self) -> Result<ExternalTransport> {
        self.validate()?;
        match &self.transport {
            ManagedEndpointTransport::Unix { socket } => {
                let socket = external_socket(socket).await?;
                let identity = platform::socket_identity(&socket).await.ok();
                Ok(ExternalTransport::Unix { socket, identity })
            }
            ManagedEndpointTransport::Ssh { address } => address
                .strip_prefix("ssh://")
                .and_then(SshEndpoint::parse)
                .map(ExternalTransport::Ssh)
                .ok_or(ManagedEndpointError::InvalidAddress),
            ManagedEndpointTransport::Tls {
                address,
                ca,
                cert,
                key,
            } => {
                let address = tls_address(address).ok_or(ManagedEndpointError::InvalidAddress)?;
                TlsEndpoint::load_files(address, ca, cert, key)
                    .await
                    .map(ExternalTransport::Tls)
                    .map_err(ManagedEndpointError::Credentials)
            }
        }
    }

    async fn validate_save(&self) -> Result<()> {
        check_user()?;
        self.validate()?;
        if let ManagedEndpointTransport::Unix { socket } = &self.transport {
            external_socket(socket).await?;
        }
        Ok(())
    }
}

async fn external_socket(path: &Path) -> Result<PathBuf> {
    let socket = fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_path_buf());
    if let Ok(local) = docker_socket_path()
        && (socket == local
            || fs::canonicalize(&local)
                .await
                .is_ok_and(|local| socket == local))
    {
        return Err(ManagedEndpointError::LocalMarsa);
    }
    Ok(socket)
}

fn tls_address(address: &str) -> Option<&str> {
    address
        .strip_prefix("tcp://")
        .or_else(|| address.strip_prefix("https://"))
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|path| valid_text(path, ADDRESS_LIMIT))
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= limit
        && !value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

fn check_user() -> Result<()> {
    if nix::unistd::geteuid().is_root() {
        return Err(ManagedEndpointError::Root);
    }
    Ok(())
}

pub(super) fn is_managed_id(id: &str) -> bool {
    id.starts_with(MANAGED_PREFIX)
}

/// The per-user file containing Marsa-managed Docker connection settings.
#[derive(Clone)]
pub struct ManagedEndpointStore {
    directory: PathBuf,
}

impl ManagedEndpointStore {
    /// Find the user's desktop state directory without launching Marsa.
    pub fn discover() -> Result<Self> {
        check_user()?;
        let socket = docker_socket_path()
            .map_err(|error| ManagedEndpointError::Store(io::Error::other(error)))?;
        let directory = socket
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| ManagedEndpointError::Store(io::ErrorKind::InvalidInput.into()))?;
        Ok(Self::at(directory.to_path_buf()))
    }

    fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// List saved settings, including endpoints that are currently unavailable.
    pub async fn list(&self) -> Result<Vec<ManagedEndpoint>> {
        check_user()?;
        match platform::check_user_docker_directory(&self.directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(ManagedEndpointError::Store(error)),
        }
        let data = match platform::read_user_docker_file(
            &self.directory.join(STORE_FILE),
            STORE_LIMIT,
            true,
        )
        .await
        {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(ManagedEndpointError::Store(error)),
        };
        let entries: Vec<ManagedEndpoint> = serde_json::from_slice(&data)
            .map_err(|error| ManagedEndpointError::Store(io::Error::other(error)))?;
        let mut ids = HashSet::new();
        if entries.len() > MAX_ENDPOINTS
            || entries.iter().any(|entry| {
                !valid_id(&entry.id) || !ids.insert(&entry.id) || entry.config.validate().is_err()
            })
        {
            return Err(ManagedEndpointError::Store(
                io::ErrorKind::InvalidData.into(),
            ));
        }
        Ok(entries)
    }

    /// Save new settings with an independent source ID. Offline endpoints may be saved.
    pub async fn add(&self, config: ManagedEndpointConfig) -> Result<ManagedEndpoint> {
        config.validate_save().await?;
        let id = request_id().map_err(ManagedEndpointError::Store)?;
        let entry = ManagedEndpoint {
            id: format!("{MANAGED_PREFIX}{id}"),
            config,
        };
        self.change(|entries| {
            if entries.len() >= MAX_ENDPOINTS || entries.iter().any(|saved| saved.id == entry.id) {
                return Err(ManagedEndpointError::Store(io::Error::other(
                    "managed endpoint store is full or allocated a duplicate identifier",
                )));
            }
            entries.push(entry.clone());
            Ok(entry)
        })
        .await
    }

    /// Replace a saved endpoint's settings while preserving its source ID.
    pub async fn edit(&self, id: &str, config: ManagedEndpointConfig) -> Result<ManagedEndpoint> {
        config.validate_save().await?;
        self.change(|entries| {
            let entry = entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or(ManagedEndpointError::NotFound)?;
            entry.config = config;
            Ok(entry.clone())
        })
        .await
    }

    /// Remove saved settings by source ID after the caller confirms removal.
    pub async fn remove(&self, id: &str) -> Result<()> {
        self.change(|entries| {
            let index = entries
                .iter()
                .position(|entry| entry.id == id)
                .ok_or(ManagedEndpointError::NotFound)?;
            entries.remove(index);
            Ok(())
        })
        .await
    }

    async fn change<T>(
        &self,
        update: impl FnOnce(&mut Vec<ManagedEndpoint>) -> Result<T>,
    ) -> Result<T> {
        check_user()?;
        let directory = self.directory.clone();
        let lock = tokio::task::spawn_blocking(move || platform::lock_docker_state(&directory))
            .await
            .map_err(|error| ManagedEndpointError::Store(io::Error::other(error)))?
            .map_err(ManagedEndpointError::Store)?;
        let mut entries = self.list().await?;
        let output = update(&mut entries)?;
        let data = serde_json::to_vec(&entries)
            .map_err(|error| ManagedEndpointError::Store(io::Error::other(error)))?;
        if data.len() > STORE_LIMIT {
            return Err(ManagedEndpointError::Store(
                io::ErrorKind::InvalidData.into(),
            ));
        }
        let path = self.directory.join(STORE_FILE);
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            platform::replace_docker_state(&path, &data)
        })
        .await
        .map_err(|error| ManagedEndpointError::Store(io::Error::other(error)))?
        .map_err(ManagedEndpointError::Store)?;
        Ok(output)
    }
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix(MANAGED_PREFIX).is_some_and(|uuid| {
        uuid.len() == 36
            && uuid.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                14 => byte == b'4',
                19 => matches!(byte, b'8' | b'9' | b'a' | b'b'),
                _ => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
            })
    })
}

#[cfg(test)]
mod tests;
