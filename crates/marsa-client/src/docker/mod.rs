//! Typed access to Docker Engine API v1.57 over Marsa's authenticated Unix socket.

mod stream;
#[cfg(test)]
mod tests;

use std::{io, path::PathBuf, time::Duration};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Method, Request, StatusCode, body::Bytes, client::conn::http1, header};
use hyper_util::rt::TokioIo;
use nix::unistd::geteuid;
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::time::timeout;

use crate::{ClientError, docker_socket_path, platform};

pub use stream::{DockerEvents, DockerLogs, DockerStats, LogLine, LogSource};

const API_PREFIX: &str = "/v1.57";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_LIMIT: usize = 8 * 1024 * 1024;
const RAW: &str = "application/vnd.docker.raw-stream";
const MULTIPLEXED: &str = "application/vnd.docker.multiplexed-stream";

/// A full container ID validated for use as a literal HTTP path segment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct ContainerId(String);

impl ContainerId {
    /// Validate a 64-character lowercase hexadecimal Docker ID.
    pub fn parse(value: impl Into<String>) -> Result<Self, DockerError> {
        let value = value.into();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            Ok(Self(value))
        } else {
            Err(DockerError::InvalidId)
        }
    }

    /// Return the complete identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContainerId {
    type Error = DockerError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// List data needed by the desktop container store.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerSummary {
    /// Complete container identifier.
    pub id: ContainerId,
    /// Docker names, including Compose-generated names.
    #[serde(default)]
    pub names: Vec<String>,
    /// Image reference reported by the engine.
    pub image: String,
    /// Machine-readable lifecycle state.
    pub state: String,
    /// Engine-provided status details.
    pub status: String,
}

/// List data needed by the desktop image store.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageSummary {
    /// Image content identifier.
    pub id: String,
    /// References that currently tag the image.
    #[serde(default, deserialize_with = "null_to_default")]
    pub repo_tags: Vec<String>,
    /// Aggregate image size in bytes.
    pub size: u64,
}

fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

/// Docker event identifying which collection has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerChange {
    /// Container metadata may have changed.
    Containers,
    /// Image metadata may have changed.
    Images,
}

/// One sample from the Docker stats stream.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatsSample {
    /// Container memory usage and limit.
    #[serde(default)]
    pub memory_stats: MemoryStats,
    /// Aggregate CPU counters.
    #[serde(default)]
    pub cpu_stats: CpuStats,
}

/// Cumulative CPU counters from Docker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CpuStats {
    /// CPU time consumed by the container.
    #[serde(default)]
    pub cpu_usage: CpuUsage,
    /// CPU time consumed by the host since boot.
    pub system_cpu_usage: Option<u64>,
}

/// CPU usage reported for the container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CpuUsage {
    /// Total container CPU time in nanoseconds.
    #[serde(default)]
    pub total_usage: u64,
}

/// Container memory counters from Docker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MemoryStats {
    /// Current total usage in bytes.
    pub usage: Option<u64>,
    /// Limit in bytes.
    pub limit: Option<u64>,
}

/// Non-destructive container lifecycle operations supported by the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    /// Start an existing container.
    Start,
    /// Stop a running container.
    Stop,
    /// Stop and start an existing container.
    Restart,
}

/// Errors returned by the Docker socket and typed response decoders.
#[derive(Debug, Error)]
pub enum DockerError {
    /// A route must be addressed by the complete container ID.
    #[error("expected a complete Docker container ID")]
    InvalidId,
    /// The transport cannot resolve a per-user socket.
    #[error(transparent)]
    Endpoint(#[from] ClientError),
    /// The local socket could not be opened or authenticated.
    #[error("could not connect to the Docker socket")]
    Socket(#[source] io::Error),
    /// Docker did not establish the request within its deadline.
    #[error("Docker response timed out")]
    Timeout,
    /// Docker returned an unsuccessful HTTP status.
    #[error("Docker returned HTTP {0}")]
    Status(StatusCode),
    /// The HTTP transport failed.
    #[error("Docker HTTP transport failed")]
    Http(#[from] hyper::Error),
    /// The typed request could not be encoded.
    #[error("invalid Docker HTTP request")]
    Request(#[from] hyper::http::Error),
    /// The Docker response body could not be read within its size limit.
    #[error("Docker snapshot exceeds the response limit or could not be read")]
    Snapshot(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The maximum size of a streaming Docker record was exceeded.
    #[error("Docker stream record exceeds the response limit")]
    RecordLimit,
    /// Docker returned an invalid or incomplete log stream.
    #[error("invalid Docker log framing")]
    LogFraming,
    /// A response did not match its data contract.
    #[error("invalid Docker JSON response")]
    Json(#[from] serde_json::Error),
}

/// Local connection details for the per-user Docker proxy.
#[derive(Debug, Clone)]
pub struct DockerClient {
    socket: PathBuf,
}

impl DockerClient {
    /// Discover Marsa's per-user socket without waking its engine.
    pub fn discover() -> Result<Self, DockerError> {
        Ok(Self::at(docker_socket_path()?))
    }

    /// Select an explicit authenticated Unix socket.
    pub fn at(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// List running and stopped containers.
    pub async fn containers(&self) -> Result<Vec<ContainerSummary>, DockerError> {
        self.snapshot("/containers/json?all=true").await
    }

    /// List all images, including untagged images.
    pub async fn images(&self) -> Result<Vec<ImageSummary>, DockerError> {
        self.snapshot("/images/json?all=true").await
    }

    /// Subscribe to future Docker container and image changes.
    pub async fn events(&self) -> Result<DockerEvents, DockerError> {
        let (_, body) = self.request(Method::GET, "/events").await?;
        Ok(DockerEvents::new(body))
    }

    /// Follow a container's output after replaying at most 200 recent lines.
    pub async fn logs(&self, id: &ContainerId) -> Result<DockerLogs, DockerError> {
        let path = format!(
            "/containers/{}/logs?stdout=true&stderr=true&follow=true&tail=200",
            id.as_str(),
        );
        let (headers, body) = self.request(Method::GET, &path).await?;
        let framed = match headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
        {
            Some(MULTIPLEXED) => true,
            Some(RAW) => false,
            _ => return Err(DockerError::LogFraming),
        };
        Ok(DockerLogs::new(body, framed))
    }

    /// Follow Docker's live statistics for one existing container.
    pub async fn stats(&self, id: &ContainerId) -> Result<DockerStats, DockerError> {
        let path = format!("/containers/{}/stats?stream=true", id.as_str());
        let (_, body) = self.request(Method::GET, &path).await?;
        Ok(DockerStats::new(body))
    }

    /// Issue a lifecycle command, leaving observed state changes to the event subscription.
    pub async fn act(&self, id: &ContainerId, action: ContainerAction) -> Result<(), DockerError> {
        let action = match action {
            ContainerAction::Start => "start",
            ContainerAction::Stop => "stop",
            ContainerAction::Restart => "restart",
        };
        let path = format!("/containers/{}/{action}", id.as_str());
        self.request(Method::POST, &path).await?;
        Ok(())
    }

    async fn snapshot<T: DeserializeOwned>(&self, path: &str) -> Result<T, DockerError> {
        timeout(REQUEST_TIMEOUT, async {
            let (_, body) = self.request(Method::GET, path).await?;
            let bytes = Limited::new(body.body, SNAPSHOT_LIMIT)
                .collect()
                .await
                .map_err(DockerError::Snapshot)?
                .to_bytes();
            Ok(serde_json::from_slice(&bytes)?)
        })
        .await
        .map_err(|_| DockerError::Timeout)?
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
    ) -> Result<(hyper::HeaderMap, stream::ResponseBody), DockerError> {
        if geteuid().is_root() {
            return Err(ClientError::Root.into());
        }
        timeout(REQUEST_TIMEOUT, async {
            let socket = platform::open_socket(self.socket.clone())
                .await
                .map_err(DockerError::Socket)?;
            let (mut sender, connection) = http1::Builder::new()
                .max_buf_size(16 * 1024)
                .handshake(TokioIo::new(socket))
                .await?;
            let task = stream::ConnectionTask::new(tokio::spawn(connection).abort_handle());
            let request = Request::builder()
                .method(method.clone())
                .uri(format!("{API_PREFIX}{path}"))
                .header(header::HOST, "docker")
                .header(header::CONNECTION, "close")
                .body(Empty::<Bytes>::new())?;
            let response = sender.send_request(request).await?;
            let (parts, body) = response.into_parts();
            if !parts.status.is_success()
                && !(method == Method::POST && parts.status == StatusCode::NOT_MODIFIED)
            {
                return Err(DockerError::Status(parts.status));
            }
            Ok((parts.headers, stream::ResponseBody::new(body, task)))
        })
        .await
        .map_err(|_| DockerError::Timeout)?
    }
}
