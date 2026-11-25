//! Typed projections for container details, volume and network screens.

use std::{collections::BTreeMap, fmt};

use hyper::Method;
use marsa_proto::host::v1::DockerOverviewSection as Section;
use serde::{Deserialize, Serialize};

use crate::MarsaClient;

use super::{
    ContainerId, ContainerSummary, DockerClient, DockerError, ImageSummary, null_to_default,
};

impl DockerClient {
    /// Read typed details and verify their identity before showing them to a view.
    pub async fn inspect(&self, id: &ContainerId) -> Result<ContainerDetails, DockerError> {
        let InspectWire {
            mut details,
            config,
        } = self.inspect_wire(id).await?;
        if let Some(config) = config {
            details.environment = config
                .env
                .iter()
                .filter_map(|entry| {
                    let (name, value) = env_parts(entry);
                    valid_environment_name(name).then(|| EnvironmentEntry {
                        name: name.to_owned(),
                        has_value: value.is_some(),
                    })
                })
                .collect();
            details.label_names = config.labels.into_keys().collect();
        }
        Ok(details)
    }

    /// Fetch only the explicitly revealed variable; the other values are discarded.
    pub async fn reveal_environment(
        &self,
        reveal: EnvironmentReveal,
    ) -> Result<Option<RevealedEnvironment>, DockerError> {
        let inspect = self.inspect_wire(&reveal.container).await?;
        let value = inspect
            .config
            .into_iter()
            .flat_map(|config| config.env)
            .find_map(|entry| {
                let (name, value) = env_parts(&entry);
                if name == reveal.name {
                    value.map(str::to_owned)
                } else {
                    None
                }
            });
        Ok(value.map(RevealedEnvironment))
    }

    async fn inspect_wire(&self, id: &ContainerId) -> Result<InspectWire, DockerError> {
        let path = format!("/containers/{}/json", id.as_str());
        let inspect: InspectWire = self.snapshot(&path).await?;
        if inspect.details.id != *id {
            return Err(DockerError::ResourceIdentity);
        }
        Ok(inspect)
    }

    /// Remove a container after explicit owner confirmation.
    pub async fn remove_container(&self, removal: ContainerRemoval) -> Result<(), DockerError> {
        let path = format!(
            "/containers/{}?force={}",
            removal.id.as_str(),
            removal.mode == RemovalMode::Force,
        );
        self.request(Method::DELETE, &path).await?;
        Ok(())
    }

    /// List volumes known to Docker.
    pub async fn volumes(&self) -> Result<Vec<VolumeSummary>, DockerError> {
        Ok(self.snapshot::<Volumes>("/volumes").await?.volumes)
    }

    /// Measure per-volume use on demand, without scanning unrelated resources.
    pub async fn volume_usage(&self) -> Result<BTreeMap<String, VolumeUsage>, DockerError> {
        let disk: DiskUsage = self.snapshot("/system/df?type=volume&verbose=true").await?;
        Ok(disk
            .volume_usage
            .into_iter()
            .flat_map(|usage| usage.items)
            .filter_map(|item| {
                item.usage_data.map(|data| {
                    (
                        item.name,
                        VolumeUsage {
                            size_bytes: u64::try_from(data.size).ok(),
                            references: u64::try_from(data.ref_count).ok(),
                        },
                    )
                })
            })
            .collect())
    }

    /// List networks known to Docker.
    pub async fn networks(&self) -> Result<Vec<NetworkSummary>, DockerError> {
        self.snapshot("/networks").await
    }
}

/// Lists that make up the managed engine's inventory.
#[derive(Default)]
pub struct DockerInventory {
    /// Running and stopped containers.
    pub containers: Vec<ContainerSummary>,
    /// All images, including untagged images.
    pub images: Vec<ImageSummary>,
    /// Known Docker volumes.
    pub volumes: Vec<VolumeSummary>,
    /// Known Docker networks.
    pub networks: Vec<NetworkSummary>,
}

impl MarsaClient {
    /// Read the managed inventory through the owner API without activating the engine.
    pub async fn docker_inventory(
        &self,
        prefer_cache: bool,
    ) -> Result<DockerInventory, DockerError> {
        Ok(DockerInventory {
            containers: self
                .read_docker_snapshot(Section::Containers, None, prefer_cache)
                .await?,
            images: self
                .read_docker_snapshot(Section::Images, None, prefer_cache)
                .await?,
            volumes: self
                .read_docker_snapshot::<Volumes>(Section::Volumes, None, prefer_cache)
                .await?
                .volumes,
            networks: self
                .read_docker_snapshot(Section::Networks, None, prefer_cache)
                .await?,
        })
    }
}

/// Explicitly chosen container removal mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalMode {
    /// Reject removal if the container is still running.
    StoppedOnly,
    /// Stop a running container and remove it.
    Force,
}

/// Removal of a selected container after its confirmation dialog succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRemoval {
    id: ContainerId,
    mode: RemovalMode,
}

impl ContainerRemoval {
    /// Record explicit user confirmation for the chosen container and mode.
    pub fn confirmed(id: ContainerId, mode: RemovalMode) -> Self {
        Self { id, mode }
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
    /// Environment names, with values masked by default.
    #[serde(skip)]
    pub environment: Vec<EnvironmentEntry>,
    /// Labels in the container configuration, without their values.
    #[serde(skip)]
    pub label_names: Vec<String>,
}

impl ContainerDetails {
    /// Encode the inspected metadata while excluding all environment and label values.
    pub fn redacted_inspect_json(&self) -> Result<String, serde_json::Error> {
        let environment: Vec<&str> = self
            .environment
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "Id": self.id.as_str(),
            "Name": self.name,
            "Image": self.image,
            "Created": self.created,
            "State": self.state,
            "Mounts": self.mounts,
            "NetworkSettings": self.network_settings,
            "Config": {
                "EnvironmentNames": environment,
                "LabelNames": self.label_names,
            },
        }))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectWire {
    #[serde(flatten)]
    details: ContainerDetails,
    config: Option<InspectConfiguration>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectConfiguration {
    #[serde(default, deserialize_with = "null_to_default")]
    env: Vec<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    labels: BTreeMap<String, serde::de::IgnoredAny>,
}

/// Environment field for ordinary rendering with its value omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentEntry {
    /// Name reported by the engine.
    pub name: String,
    /// Whether a value exists and can be revealed.
    pub has_value: bool,
}

/// A request authorized by an explicit reveal action in the selected container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentReveal {
    container: ContainerId,
    name: String,
}

impl EnvironmentReveal {
    /// Record the variable and container after the owner deliberately reveals it.
    pub fn confirmed(container: ContainerId, name: impl Into<String>) -> Result<Self, DockerError> {
        let name = name.into();
        if !valid_environment_name(&name) {
            return Err(DockerError::InvalidEnvironmentName);
        }
        Ok(Self { container, name })
    }
}

/// Secret value scoped to the revealing view. Debug output always omits it.
pub struct RevealedEnvironment(String);

impl RevealedEnvironment {
    /// Read a revealed value for its transient presentation.
    pub fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RevealedEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevealedEnvironment")
            .finish_non_exhaustive()
    }
}

fn valid_environment_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 4096 && !name.chars().any(|ch| ch.is_control() || ch == '=')
}

fn env_parts(entry: &str) -> (&str, Option<&str>) {
    entry
        .split_once('=')
        .map_or((entry, None), |(name, value)| (name, Some(value)))
}

/// Runtime portion of an inspect response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PortBinding {
    /// Address chosen for the published port.
    #[serde(rename = "HostIp")]
    pub host_ip: String,
    /// Host port in the engine's original string representation.
    #[serde(rename = "HostPort")]
    pub host_port: String,
}

/// Current volume usage collected by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeUsage {
    /// Disk consumption when the local volume driver can measure it.
    pub size_bytes: Option<u64>,
    /// Number of container references when Docker can report it.
    pub references: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskUsage {
    volume_usage: Option<VolumeUsageList>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeUsageList {
    #[serde(default, deserialize_with = "null_to_default")]
    items: Vec<VolumeUsageItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeUsageItem {
    name: String,
    usage_data: Option<UsageData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UsageData {
    size: i64,
    ref_count: i64,
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
