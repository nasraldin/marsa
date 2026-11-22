//! Typed projections for container details, volume and network screens.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{ContainerId, DockerClient, DockerError, null_to_default};

impl DockerClient {
    /// Read typed details and verify their identity before showing them to a view.
    pub async fn inspect(&self, id: &ContainerId) -> Result<ContainerDetails, DockerError> {
        let path = format!("/containers/{}/json", id.as_str());
        let details: ContainerDetails = self.snapshot(&path).await?;
        if details.id != *id {
            return Err(DockerError::ResourceIdentity);
        }
        Ok(details)
    }

    /// List volumes known to Docker.
    pub async fn volumes(&self) -> Result<Vec<VolumeSummary>, DockerError> {
        Ok(self.snapshot::<Volumes>("/volumes").await?.volumes)
    }

    /// List networks known to Docker.
    pub async fn networks(&self) -> Result<Vec<NetworkSummary>, DockerError> {
        self.snapshot("/networks").await
    }
}

#[derive(Deserialize)]
struct Volumes {
    #[serde(rename = "Volumes", default, deserialize_with = "null_to_default")]
    volumes: Vec<VolumeSummary>,
}

/// Published or exposed port reported in the container list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerPort {
    /// Host address for a published binding.
    #[serde(rename = "IP")]
    pub host_ip: Option<String>,
    /// Docker container port.
    pub private_port: u16,
    /// Host port, if this port is published.
    pub public_port: Option<u16>,
    /// Transport protocol such as TCP or UDP.
    #[serde(rename = "Type")]
    pub protocol: String,
}

/// One mount reported for a container.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerMount {
    /// Driver's type such as volume, bind or tmpfs.
    #[serde(rename = "Type")]
    pub kind: String,
    /// Docker volume name where applicable.
    pub name: Option<String>,
    /// Engine-reported mount source.
    #[serde(default)]
    pub source: String,
    /// Mount destination inside the container.
    pub destination: String,
    /// Whether the mount permits writes.
    #[serde(rename = "RW", default)]
    pub writable: bool,
}

/// Container details suitable for ordinary presentation without environment values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerDetails {
    /// The inspected container ID.
    pub id: ContainerId,
    /// Container name reported by the engine.
    pub name: String,
    /// The image content identifier.
    pub image: String,
    /// Creation timestamp in Docker's original format.
    pub created: String,
    /// Docker runtime state when present.
    pub state: Option<ContainerRuntime>,
    /// Mounts attached to this container.
    #[serde(default, deserialize_with = "null_to_default")]
    pub mounts: Vec<ContainerMount>,
    /// Network endpoints and port mappings, if available.
    pub network_settings: Option<ContainerNetworkSettings>,
}

/// Runtime portion of an inspect response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerRuntime {
    /// Machine-readable lifecycle state.
    pub status: String,
    /// Whether the workload currently runs.
    pub running: bool,
    /// Whether the engine has paused the workload.
    pub paused: bool,
    /// Most recent process exit status.
    pub exit_code: i32,
    /// Last workload start in the engine's timestamp format.
    pub started_at: String,
    /// Last workload exit in the engine's timestamp format.
    pub finished_at: String,
}

/// Container network attachments and detailed host port mappings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerNetworkSettings {
    /// Networks indexed by Docker network name.
    #[serde(default, deserialize_with = "null_to_default")]
    pub networks: BTreeMap<String, NetworkEndpoint>,
    /// Bindings indexed by container port and protocol, such as `443/tcp`.
    #[serde(default, deserialize_with = "null_to_default")]
    pub ports: BTreeMap<String, Option<Vec<PortBinding>>>,
}

/// Network attachment for a container.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkEndpoint {
    /// Stable Docker network ID.
    #[serde(rename = "NetworkID")]
    pub network_id: String,
    /// Assigned IPv4 address or an empty value for inactive endpoints.
    #[serde(rename = "IPAddress", default)]
    pub ipv4: String,
    /// Assigned IPv6 address or an empty value for inactive endpoints.
    #[serde(rename = "GlobalIPv6Address", default)]
    pub ipv6: String,
    /// Gateway address at this endpoint.
    #[serde(default)]
    pub gateway: String,
}

/// Published host binding reported by `docker inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PortBinding {
    /// Address chosen for the published port.
    #[serde(rename = "HostIp")]
    pub host_ip: String,
    /// Host port in the engine's original string representation.
    #[serde(rename = "HostPort")]
    pub host_port: String,
}

/// Local or remote volume metadata reported by Docker.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumeSummary {
    /// Canonical volume name.
    pub name: String,
    /// Volume driver name.
    pub driver: String,
    /// Driver's scope.
    pub scope: String,
    /// Creation timestamp when supported by the driver.
    pub created_at: Option<String>,
}

/// Docker network metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkSummary {
    /// Complete network identifier.
    pub id: String,
    /// Display name chosen when the network was created.
    pub name: String,
    /// Network driver.
    pub driver: String,
    /// Driver's scope.
    pub scope: String,
    /// Whether the network is restricted to its members.
    #[serde(default)]
    pub internal: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(super) struct ComposeLabels {
    #[serde(rename = "com.docker.compose.project")]
    pub(super) project: Option<String>,
    #[serde(rename = "com.docker.compose.service")]
    pub(super) service: Option<String>,
}
