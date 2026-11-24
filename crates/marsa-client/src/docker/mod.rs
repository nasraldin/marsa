//! Typed access to Marsa and authenticated external Docker engines.

mod actions;
mod contexts;
mod environment_report;
mod hawser;
mod images;
mod managed;
mod registry;
mod resources;
pub mod scanning;
mod stats;
mod stream;
mod terminal;
#[cfg(test)]
mod tests;
mod transfer;

use std::{io, path::PathBuf, time::Duration};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, StatusCode, body::Bytes, client::conn::http1, header};
use hyper_util::rt::TokioIo;
use nix::unistd::geteuid;
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};

use crate::{ClientError, docker_socket_path, platform};

mod container_files;
mod inventory;
mod lima;
mod overview;
pub use container_files::{ContainerFileFormat, ContainerPath};
pub use environment_report::{DockerActivity, EnvironmentReport, EnvironmentReportContainer};
pub use hawser::HAWSER_EDGE_PATH;
pub use inventory::{
    ExternalEndpoint, ExternalInventory, discover_external_inventories, discover_external_inventory,
};
pub use lima::{LimaInstance, LimaSettings, LimaSettingsError, LimaSettingsUpdate};
pub use managed::{
    ManagedEndpoint, ManagedEndpointConfig, ManagedEndpointError, ManagedEndpointStore,
    ManagedEndpointTransport, VerifiedEngine,
};
pub use registry::{
    RegistryAddress, RegistryCredentials, RegistryEntry, RegistryError, RegistryStore,
};
pub use transfer::NetworkId;

/// Versioned container creation records shared with the owner daemon.
pub use marsa_proto::host::container_config;

pub use actions::{
    CreatedNetwork, ImageDetails, ImageId, ImageRemoval, NetworkRemoval, ResourceName,
    VolumeRemoval,
};
pub use images::{
    ContainerMountReview, ImagePull, ImageReference, ImageRun, PullCounters, PullProgress,
};
pub use overview::{
    ContainerCounts, ContainerUtilization, DiskBreakdown, EngineVersion, OverviewClient,
    OverviewSample, OverviewUtilization,
};
use resources::ComposeLabels;
pub use resources::{
    ContainerDetails, ContainerMount, ContainerNetworkSettings, ContainerPort, ContainerRemoval,
    ContainerRuntime, EnvironmentEntry, EnvironmentReveal, NetworkEndpoint, NetworkSummary,
    PortBinding, RemovalMode, RevealedEnvironment, VolumeSummary, VolumeUsage,
};
pub use stats::{
    BlockIoCounter, BlockIoStats, CpuStats, CpuUsage, MemoryStats, NetworkCounters, ProcessStats,
    StatsSample,
};
pub use stream::{DockerEvents, DockerLogs, DockerStats, LogLine, LogSource, LogTimestamp};
pub use terminal::{ExecControl, ExecOutput, ExecSession, MAX_TERMINAL_INPUT, TerminalSize};

const API_PREFIX: &str = "/v1.57";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_LIMIT: usize = 8 * 1024 * 1024;
const LOG_ENDPOINT: &str = "logs?stdout=true&stderr=true&follow=true&tail=200&timestamps=true";
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
    #[serde(default, deserialize_with = "null_to_default")]
    pub names: Vec<String>,
    /// Image reference reported by the engine.
    pub image: String,
    /// Machine-readable lifecycle state.
    pub state: String,
    /// Engine-provided status details.
    pub status: String,
    health: Option<ContainerHealth>,
    /// Creation time in Unix seconds.
    pub created: i64,
    /// Stable image identifier from Docker.
    #[serde(rename = "ImageID")]
    pub image_id: String,
    /// Ports published or exposed by this container.
    #[serde(default, deserialize_with = "null_to_default")]
    pub ports: Vec<ContainerPort>,
    /// Mounts used to derive volume membership.
    #[serde(default, deserialize_with = "null_to_default")]
    pub mounts: Vec<ContainerMount>,
    /// Network memberships and endpoint details.
    pub network_settings: Option<ContainerNetworkSettings>,
    #[serde(rename = "Labels", default, deserialize_with = "null_to_default")]
    compose: ComposeLabels,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ContainerHealth {
    #[serde(rename = "Status", default)]
    status: String,
}

impl ContainerSummary {
    /// Machine-readable lifecycle state, such as `running` or `paused`.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Engine health check outcome, such as `healthy` or `unhealthy`.
    pub fn health(&self) -> Option<&str> {
        self.health.as_ref().map(|health| health.status.as_str())
    }

    /// Compose project label when this container is a project member.
    pub fn compose_project(&self) -> Option<&str> {
        self.compose
            .project
            .as_deref()
            .filter(|name| !name.is_empty())
    }

    /// Compose service label when this container belongs to a service.
    pub fn compose_service(&self) -> Option<&str> {
        self.compose
            .service
            .as_deref()
            .filter(|name| !name.is_empty())
    }
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
    /// Creation time in Unix seconds.
    pub created: i64,
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

/// Non-destructive container lifecycle operations supported by the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    /// Start an existing container.
    Start,
    /// Stop a running container.
    Stop,
    /// Stop and start an existing container.
    Restart,
    /// Pause the processes in a running container.
    Pause,
    /// Resume the processes in a paused container.
    Unpause,
}

/// Errors returned by the Docker socket and typed response decoders.
#[derive(Debug, Error)]
pub enum DockerError {
    /// A route must be addressed by the complete container ID.
    #[error("expected a complete Docker container ID")]
    InvalidId,
    /// Docker resource name or reference isn't supported.
    #[error("invalid Docker resource name or image reference")]
    InvalidResourceName,
    /// Container options did not satisfy the constrained create contract.
    #[error(transparent)]
    InvalidContainerConfig(#[from] container_config::ContainerConfigError),
    /// The user did not confirm host paths for the selected external engine.
    #[error("review external host mounts before creating the container")]
    ExternalBindReviewRequired,
    /// A volume removal was not confirmed with its exact name.
    #[error("volume confirmation did not match")]
    VolumeConfirmation,
    /// An environment reveal selected an invalid variable name.
    #[error("invalid Docker environment name")]
    InvalidEnvironmentName,
    /// The registry did not provide an image for the configured guest platform.
    #[error("image has no matching Linux ARM64 manifest")]
    ImagePlatform,
    /// Docker reported failure while pulling an image.
    #[error("Docker image pull failed")]
    ImagePull,
    /// Docker rejected a registry push.
    #[error("Docker image push failed")]
    ImagePush,
    /// Docker rejected an image archive import.
    #[error("Docker image load failed")]
    ImageLoad,
    /// The requested container path is invalid.
    #[error("container file requires an absolute path")]
    ContainerFilePath,
    /// A container archive could not be received or written.
    #[error("could not download the selected container file")]
    ContainerArchive(#[source] io::Error),
    /// An image archive could not be read or written.
    #[error("Docker image archive I/O failed")]
    ImageArchive(#[source] io::Error),
    /// Docker registry authentication could not be obtained.
    #[error("Docker registry authentication unavailable")]
    RegistryAuthentication,
    /// A terminal dimension was zero or exceeded the supported bound.
    #[error("invalid terminal dimensions")]
    TerminalSize,
    /// The engine did not negotiate the requested raw TTY protocol.
    #[error("invalid Docker terminal upgrade")]
    TerminalUpgrade,
    /// An interactive stream could not read or write bytes.
    #[error("Docker terminal I/O failed")]
    TerminalIo(#[source] io::Error),
    /// The transport cannot resolve a per-user socket.
    #[error(transparent)]
    Endpoint(#[from] ClientError),
    /// The pinned endpoint could not be opened or authenticated.
    #[error("could not connect to the Docker endpoint")]
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
    /// An inspected resource did not match the requested identifier.
    #[error("Docker inspect returned a different resource")]
    ResourceIdentity,
    /// A response did not match its data contract.
    #[error("invalid Docker JSON response")]
    Json(#[from] serde_json::Error),
}

pub(crate) trait DockerIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> DockerIo for T {}
pub(crate) type DockerTransport = Box<dyn DockerIo>;

/// Connection details pinned to a Docker engine.
#[derive(Debug, Clone)]
pub struct DockerClient {
    scope: DockerScope,
}

#[derive(Debug, Clone)]
enum DockerScope {
    Marsa(PathBuf),
    External {
        endpoint: ExternalEndpoint,
        containers: bool,
        probe_identity: bool,
    },
}

impl DockerClient {
    /// Discover Marsa's per-user socket without waking its engine.
    pub fn discover() -> Result<Self, DockerError> {
        Ok(Self::at(docker_socket_path()?))
    }

    /// Select an explicit authenticated Unix socket.
    pub fn at(socket: impl Into<PathBuf>) -> Self {
        Self {
            scope: DockerScope::Marsa(socket.into()),
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
        let path = format!("/containers/{}/{LOG_ENDPOINT}", id.as_str());
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
            ContainerAction::Pause => "pause",
            ContainerAction::Unpause => "unpause",
        };
        let path = format!("/containers/{}/{action}", id.as_str());
        self.request(Method::POST, &path).await?;
        Ok(())
    }

    async fn snapshot<T: DeserializeOwned>(&self, path: &str) -> Result<T, DockerError> {
        self.snapshot_with_timeout(path, REQUEST_TIMEOUT).await
    }

    async fn snapshot_with_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        deadline: Duration,
    ) -> Result<T, DockerError> {
        timeout(deadline, async {
            let (_, body) = self
                .request_body(
                    Method::GET,
                    path,
                    Full::new(Bytes::new()),
                    Default::default(),
                    deadline,
                )
                .await?;
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

    async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        payload: serde_json::Value,
    ) -> Result<T, DockerError> {
        timeout(REQUEST_TIMEOUT, async {
            let (_, body) = self
                .request_payload(Method::POST, path, serde_json::to_vec(&payload)?.into())
                .await?;
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
        self.request_payload(method, path, Bytes::new()).await
    }

    fn request_uri(&self, path: &str) -> String {
        match self.scope {
            DockerScope::Marsa(_) => format!("{API_PREFIX}{path}"),
            DockerScope::External { .. } => path.to_owned(),
        }
    }

    fn request_host(&self) -> &str {
        match &self.scope {
            DockerScope::Marsa(_) => "docker",
            DockerScope::External { endpoint, .. } => endpoint.request_host(),
        }
    }

    async fn request_socket(
        &self,
        method: &Method,
        path: &str,
        has_body: bool,
    ) -> Result<DockerTransport, DockerError> {
        if geteuid().is_root() {
            return Err(ClientError::Root.into());
        }
        match &self.scope {
            DockerScope::Marsa(socket) => {
                let transport = platform::open_socket(socket.clone())
                    .await
                    .map_err(DockerError::Socket)?;
                Ok(Box::new(transport))
            }
            DockerScope::External {
                endpoint,
                containers,
                probe_identity,
            } => {
                let identity_probe =
                    *probe_identity && *method == Method::GET && path == "/info" && !has_body;
                if !identity_probe
                    && !inventory::external_route(method, path, *containers, has_body)
                {
                    return Err(DockerError::Socket(io::ErrorKind::PermissionDenied.into()));
                }
                endpoint.open_transport().await.map_err(DockerError::Socket)
            }
        }
    }

    async fn request_payload(
        &self,
        method: Method,
        path: &str,
        payload: Bytes,
    ) -> Result<(hyper::HeaderMap, stream::ResponseBody), DockerError> {
        let deadline = if matches!(
            self.scope,
            DockerScope::External {
                containers: true,
                ..
            }
        ) && method == Method::POST
            && path.ends_with("/stop")
        {
            Duration::from_secs(60)
        } else {
            REQUEST_TIMEOUT
        };
        let mut headers = hyper::HeaderMap::new();
        if !payload.is_empty() {
            headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
        }
        self.request_body(method, path, Full::new(payload), headers, deadline)
            .await
    }

    async fn request_body<B>(
        &self,
        method: Method,
        path: &str,
        body: B,
        headers: hyper::HeaderMap,
        deadline: Duration,
    ) -> Result<(hyper::HeaderMap, stream::ResponseBody), DockerError>
    where
        B: hyper::body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        timeout(deadline, async {
            let socket = self
                .request_socket(&method, path, !body.is_end_stream())
                .await?;
            let (mut sender, connection) = http1::Builder::new()
                .max_buf_size(16 * 1024)
                .handshake(TokioIo::new(socket))
                .await?;
            let task = stream::ConnectionTask::new(tokio::spawn(connection).abort_handle());
            let mut request = Request::builder()
                .method(method.clone())
                .uri(self.request_uri(path))
                .header(header::HOST, self.request_host())
                .header(header::CONNECTION, "close")
                .body(body)?;
            request.headers_mut().extend(headers);
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
