//! Read-only dashboard projections scoped to one Docker engine.

use std::{collections::BTreeSet, time::Duration};

use marsa_proto::host::v1::{DockerOverviewRequest, DockerOverviewSection as Section};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::{task::JoinSet, time::sleep};

use crate::MarsaClient;

use super::{
    ContainerId, ContainerSummary, DockerClient, ExternalEndpoint, ImageSummary, NetworkSummary,
    StatsSample, VolumeSummary, environment_report::display_value,
};

const OVERVIEW_API_MINOR: u32 = 13;
const SAMPLE_LIMIT: usize = 48;
const SAMPLE_CONCURRENCY: usize = 8;
const SAMPLE_BUDGET: Duration = Duration::from_secs(8);
const DISK_TIMEOUT: Duration = Duration::from_secs(40);
pub(super) const DISK_PATH: &str = "/system/df?verbose=true";
pub(super) const VERSION_PATH: &str = "/version";
pub(super) const STATS_PATH: &str = "stats?stream=false";

/// Selects the authenticated transport used for a dashboard refresh.
#[derive(Clone)]
pub enum OverviewClient {
    /// Uses the daemon's passive API, which only connects to an already-running guest.
    Marsa(MarsaClient),
    /// Uses a verified external Docker endpoint.
    External(DockerClient),
}

impl OverviewClient {
    /// Capture the endpoint so a refresh cannot follow a later environment selection.
    pub fn external(endpoint: &ExternalEndpoint) -> Self {
        Self::External(endpoint.management_client())
    }

    /// Read the inventory and live container measurements.
    pub async fn sample(&self) -> OverviewSample {
        let containers = async {
            let rows: Option<Vec<ContainerSummary>> = self.read(Section::Containers, None).await;
            let Some(rows) = rows else {
                return (None, None, None);
            };
            let counts = ContainerCounts::from_rows(&rows);
            let stacks = rows
                .iter()
                .filter_map(ContainerSummary::compose_project)
                .collect::<BTreeSet<_>>()
                .len();
            let utilization = self.utilization(&rows).await;
            (Some(counts), Some(stacks), Some(utilization))
        };
        let images = self.read::<Vec<ImageSummary>>(Section::Images, None);
        let networks = self.read::<Vec<NetworkSummary>>(Section::Networks, None);
        let volumes = self.read::<Volumes>(Section::Volumes, None);
        let version = self.read::<EngineVersion>(Section::Version, None);
        let ((containers, stacks, utilization), images, volumes, networks, version) =
            tokio::join!(containers, images, volumes, networks, version);
        OverviewSample {
            containers,
            stacks,
            images: images.map(|rows| rows.len()),
            volumes: volumes.map(|result| result.volumes.len()),
            networks: networks.map(|rows| rows.len()),
            version: version.and_then(EngineVersion::checked),
            utilization,
        }
    }

    /// Measure storage separately because the engine may need to scan its filesystem.
    pub async fn disk(&self) -> Option<DiskBreakdown> {
        self.read::<EngineDiskUsage>(Section::Disk, None)
            .await
            .map(EngineDiskUsage::project)
            .filter(DiskBreakdown::has_measurements)
    }

    async fn utilization(&self, rows: &[ContainerSummary]) -> OverviewUtilization {
        let mut rows = rows.iter().filter(|row| row.state == "running");
        let running = rows.clone().count();
        let mut samples = JoinSet::new();
        let mut usage = Vec::new();
        let deadline = sleep(SAMPLE_BUDGET);
        tokio::pin!(deadline);
        for _ in 0..SAMPLE_LIMIT.min(running) {
            if samples.len() >= SAMPLE_CONCURRENCY {
                tokio::select! {
                    biased;
                    () = &mut deadline => break,
                    result = samples.join_next() => push_usage(&mut usage, result),
                }
            }
            let Some(row) = rows.next() else {
                break;
            };
            let id = row.id.clone();
            let name = row
                .names
                .first()
                .map(|name| name.trim_start_matches('/'))
                .filter(|name| !name.is_empty())
                .unwrap_or(&id.as_str()[..12]);
            let name = display_value(name, 255);
            let client = self.clone();
            samples.spawn(async move {
                let stats: StatsSample = client.read(Section::ContainerStats, Some(&id)).await?;
                let cpu_percent = stats.cpu_percent();
                let memory_bytes = stats.memory_stats.used_without_cache();
                (cpu_percent.is_some() || memory_bytes.is_some()).then_some(ContainerUtilization {
                    id,
                    name,
                    cpu_percent,
                    memory_bytes,
                })
            });
        }
        while !samples.is_empty() {
            tokio::select! {
                biased;
                () = &mut deadline => break,
                result = samples.join_next() => push_usage(&mut usage, result),
            }
        }
        OverviewUtilization { running, usage }
    }

    async fn read<T: DeserializeOwned>(
        &self,
        section: Section,
        container: Option<&ContainerId>,
    ) -> Option<T> {
        let deadline = if section == Section::Disk {
            DISK_TIMEOUT
        } else {
            super::REQUEST_TIMEOUT
        };
        match self {
            Self::Marsa(client) if client.api_minor() >= OVERVIEW_API_MINOR => {
                let mut request = tonic::Request::new(DockerOverviewRequest {
                    section: section.into(),
                    container_id: container.map_or_else(String::new, |id| id.as_str().to_owned()),
                });
                request.set_timeout(deadline);
                let response = client
                    .stats()
                    .docker_overview(request)
                    .await
                    .ok()?
                    .into_inner();
                serde_json::from_slice(&response.json).ok()
            }
            Self::Marsa(_) => None,
            Self::External(client) => {
                let path = match section {
                    Section::Containers => "/containers/json?all=true".into(),
                    Section::Images => "/images/json?all=true".into(),
                    Section::Volumes => "/volumes".into(),
                    Section::Networks => "/networks".into(),
                    Section::Version => VERSION_PATH.into(),
                    Section::Disk => DISK_PATH.into(),
                    Section::ContainerStats => {
                        format!("/containers/{}/{STATS_PATH}", container?.as_str())
                    }
                    Section::Unspecified => return None,
                };
                client.snapshot_with_timeout(&path, deadline).await.ok()
            }
        }
    }
}

fn push_usage(
    usage: &mut Vec<ContainerUtilization>,
    result: Option<Result<Option<ContainerUtilization>, tokio::task::JoinError>>,
) {
    if let Some(Ok(Some(result))) = result {
        usage.push(result);
    }
}

/// Independent observations from a single selected Docker engine.
#[derive(Debug, Default)]
pub struct OverviewSample {
    /// Lifecycle and health counts, or no counts when inventory could not be read.
    pub containers: Option<ContainerCounts>,
    /// Number of distinct Compose projects labelled on containers.
    pub stacks: Option<usize>,
    /// Number of image records on this engine.
    pub images: Option<usize>,
    /// Number of volume records on this engine.
    pub volumes: Option<usize>,
    /// Number of network records on this engine.
    pub networks: Option<usize>,
    /// Version and platform reported by the selected Docker engine.
    pub version: Option<EngineVersion>,
    /// CPU and memory measurement availability and results.
    pub utilization: Option<OverviewUtilization>,
}

/// Counts of lifecycle and health states from a single listing.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ContainerCounts {
    /// All listed containers.
    pub total: usize,
    /// Containers in the running state.
    pub running: usize,
    /// Created, exited, or dead containers.
    pub stopped: usize,
    /// Paused containers.
    pub paused: usize,
    /// Containers that are restarting.
    pub restarting: usize,
    /// Containers with another lifecycle state, such as removal in progress.
    pub other: usize,
    /// Containers reporting healthy.
    pub healthy: usize,
    /// Containers reporting unhealthy.
    pub unhealthy: usize,
    /// Containers whose health check is still starting.
    pub health_starting: usize,
    /// Containers reporting that no health status applies.
    pub no_health: usize,
    /// Containers without a usable health status in the response.
    pub unknown_health: usize,
}

impl ContainerCounts {
    fn from_rows(rows: &[ContainerSummary]) -> Self {
        let mut counts = Self {
            total: rows.len(),
            ..Self::default()
        };
        for row in rows {
            match row.state.as_str() {
                "running" => counts.running += 1,
                "created" | "exited" | "dead" => counts.stopped += 1,
                "paused" => counts.paused += 1,
                "restarting" => counts.restarting += 1,
                _ => counts.other += 1,
            }
            let health = row.health.as_ref().map(|health| health.status.as_str());
            match (
                health,
                row.status.rsplit_once(" (").map(|(_, suffix)| suffix),
            ) {
                (Some("healthy"), _) | (None, Some("healthy)")) => counts.healthy += 1,
                (Some("unhealthy"), _) | (None, Some("unhealthy)")) => counts.unhealthy += 1,
                (Some("starting"), _) | (None, Some("health: starting)")) => {
                    counts.health_starting += 1
                }
                (Some("none"), _) => counts.no_health += 1,
                _ => counts.unknown_health += 1,
            }
        }
        counts
    }
}

/// Sample coverage for the currently running containers.
#[derive(Debug)]
pub struct OverviewUtilization {
    /// Number of containers eligible for sampling.
    pub running: usize,
    /// Containers with at least one measurement, in no particular order.
    pub usage: Vec<ContainerUtilization>,
}

/// Measured resource use reported by Docker for one container.
#[derive(Debug)]
pub struct ContainerUtilization {
    /// Full container ID, used to break ties reproducibly.
    pub id: ContainerId,
    /// Engine name or the short ID when no name is present.
    pub name: String,
    /// Percentage of one logical CPU, if the engine supplied both counters.
    pub cpu_percent: Option<f64>,
    /// Memory in bytes excluding inactive file cache, where reported.
    pub memory_bytes: Option<u64>,
}

/// Docker Engine version and platform.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EngineVersion {
    /// Docker Engine version.
    pub version: String,
    /// Reported operating system.
    pub os: String,
    /// Reported architecture.
    pub arch: String,
}

impl EngineVersion {
    fn checked(self) -> Option<Self> {
        let valid = [&self.version, &self.os, &self.arch]
            .into_iter()
            .all(|field| super::managed::valid_text(field, 64));
        valid.then_some(self)
    }
}

/// Engine-reported category sizes, which may share underlying storage.
#[derive(Debug, Default)]
pub struct DiskBreakdown {
    /// Writable container layers.
    pub containers: Option<u64>,
    /// Image layers reported by the engine.
    pub images: Option<u64>,
    /// Volume space the engine can measure.
    pub volumes: Option<u64>,
    /// Build cache space reported by the engine.
    pub build_cache: Option<u64>,
}

impl DiskBreakdown {
    fn has_measurements(&self) -> bool {
        [self.containers, self.images, self.volumes, self.build_cache]
            .into_iter()
            .any(|size| size.is_some())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EngineDiskUsage {
    container_usage: Option<Usage>,
    image_usage: Option<Usage>,
    volume_usage: Option<Usage>,
    build_cache_usage: Option<Usage>,
    layers_size: Option<i64>,
    containers: Option<Vec<LegacyContainerUsage>>,
    volumes: Option<Vec<LegacyVolumeUsage>>,
}

impl EngineDiskUsage {
    fn project(self) -> DiskBreakdown {
        let size = |usage: Usage| u64::try_from(usage.total_size).ok();
        let containers = self
            .container_usage
            .map_or_else(|| sum_legacy(self.containers?, |row| row.size_rw), size);
        let images = self
            .image_usage
            .map_or_else(|| u64::try_from(self.layers_size?).ok(), size);
        let volumes = self.volume_usage.map_or_else(
            || sum_legacy(self.volumes?, |row| row.usage_data?.size),
            size,
        );
        // Legacy BuildCache arrays can contain shared records without a bounded aggregate.
        DiskBreakdown {
            containers,
            images,
            volumes,
            build_cache: self.build_cache_usage.and_then(size),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Usage {
    // Moby omits zero totals inside a present category.
    #[serde(default)]
    total_size: i64,
}

fn sum_legacy<T>(items: Vec<T>, size: impl Fn(T) -> Option<i64>) -> Option<u64> {
    items.into_iter().try_fold(0u64, |sum, row| {
        sum.checked_add(u64::try_from(size(row)?).ok()?)
    })
}

#[derive(Deserialize)]
struct LegacyContainerUsage {
    #[serde(rename = "SizeRw")]
    size_rw: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacyVolumeUsage {
    usage_data: Option<LegacyVolumeSize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacyVolumeSize {
    size: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Volumes {
    // Docker's empty volume list may be null. A missing key is unavailable.
    #[serde(deserialize_with = "super::null_to_default")]
    volumes: Vec<VolumeSummary>,
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use hyper::Method;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;
    use crate::{
        docker::inventory::{ExternalTransport, external_route},
        platform,
    };

    #[test]
    fn rejects_unbounded_or_non_get_overview_routes() {
        let id = "a".repeat(64);
        for route in [
            DISK_PATH,
            VERSION_PATH,
            &format!("/containers/{id}/{STATS_PATH}"),
        ] {
            assert!(external_route(&Method::GET, route, true, false));
            assert!(!external_route(&Method::GET, route, false, false));
            assert!(!external_route(&Method::GET, route, true, true));
            assert!(!external_route(&Method::POST, route, true, false));
        }
        assert!(!external_route(
            &Method::GET,
            &format!("/containers/{id}/stats?stream=false&foo=bar"),
            true,
            false,
        ));
    }

    #[test]
    fn missing_and_negative_disk_sizes_are_distinct_from_zero() {
        let disk: EngineDiskUsage = serde_json::from_str(
            r#"{"ContainerUsage":{},"ImageUsage":{"TotalSize":-1},"VolumeUsage":{"TotalSize":1024}}"#,
        ).unwrap();
        let usage = disk.project();
        assert_eq!(usage.containers, Some(0));
        assert_eq!(usage.images, None);
        assert_eq!(usage.volumes, Some(1024));
        assert_eq!(usage.build_cache, None);
        assert!(
            serde_json::from_str::<Volumes>(r#"{"Volumes":null}"#)
                .unwrap()
                .volumes
                .is_empty()
        );
        assert!(serde_json::from_str::<Volumes>("{}").is_err());
        let legacy: EngineDiskUsage = serde_json::from_str(
            r#"{"LayersSize":4096,"Containers":[{"SizeRw":1024},{"SizeRw":2048}],"Volumes":[{"UsageData":{"Size":-1}}],"BuildCache":[{"Size":100}]}"#,
        ).unwrap();
        let usage = legacy.project();
        assert_eq!(usage.images, Some(4096));
        assert_eq!(usage.containers, Some(3072));
        assert_eq!(usage.volumes, None);
        assert_eq!(usage.build_cache, None);
    }

    #[tokio::test]
    async fn projects_live_inventory_samples_and_disk_from_the_selected_socket() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let path = directory.path().join("docker.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let endpoint = ExternalEndpoint::managed(
            "managed:overview-fixture".into(),
            ExternalTransport::Unix {
                identity: Some(platform::socket_identity(&path).await.unwrap()),
                socket: path,
            },
        );
        let stats_path = format!("/containers/{}/stats?stream=false", "a".repeat(64));
        let container = |digit: &str, state: &str, status: &str, health: Option<&str>| {
            serde_json::json!({
                "Id": digit.repeat(64),
                "Names": ["/web"],
                "Image": "example/web",
                "ImageID": "sha256:l2m",
                "Created": 0,
                "State": state,
                "Status": status,
                "Health": health.map(|status| serde_json::json!({"Status": status})),
                "Labels": {"com.docker.compose.project": "example"},
            })
        };
        let inventory = serde_json::to_string(&[
            container("a", "running", "Up 2 minutes", Some("healthy")),
            container("b", "paused", "Up 1 hour (Paused)", Some("none")),
            container("c", "restarting", "Restarting", Some("none")),
            container("d", "exited", "Exited", None),
            container("e", "removing", "", None),
        ])
        .unwrap();
        let routes = std::collections::BTreeMap::from([
            ("/containers/json?all=true".into(), inventory),
            ("/images/json?all=true".into(), r#"[{"Id":"sha256:first","Created":0,"Size":128},{"Id":"sha256:second","Created":0,"Size":256}]"#.into()),
            ("/volumes".into(), r#"{"Volumes":[{"Name":"web-data","Driver":"local","Scope":"local"}]}"#.into()),
            ("/networks".into(), "[]".into()),
            (VERSION_PATH.into(), r#"{"Version":"29.10.0","Os":"linux","Arch":"arm64"}"#.into()),
            (DISK_PATH.into(), r#"{"ImageUsage":{"TotalSize":4096},"ContainerUsage":{},"VolumeUsage":{"TotalSize":512},"BuildCacheUsage":{"TotalSize":1024}}"#.into()),
            (stats_path, r#"{"memory_stats":{"usage":4096,"stats":{"inactive_file":1024}},"cpu_stats":{"system_cpu_usage":130,"cpu_usage":{"total_usage":18},"online_cpus":2},"precpu_stats":{"system_cpu_usage":100,"cpu_usage":{"total_usage":12}}}"#.into()),
        ]);
        let server = tokio::spawn(async move {
            let mut replies = JoinSet::new();
            let routes = std::sync::Arc::new(routes);
            for _ in 0..routes.len() {
                let (mut socket, _) = listener.accept().await.unwrap();
                let routes = routes.clone();
                replies.spawn(async move {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.lines().next().unwrap().strip_prefix("GET ").unwrap().strip_suffix(" HTTP/1.1").unwrap();
                    let body = routes.get(path).unwrap();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len(),
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                });
            }
            while let Some(result) = replies.join_next().await {
                result.unwrap();
            }
        });
        let client = OverviewClient::external(&endpoint);
        let (sample, disk) = tokio::join!(client.sample(), client.disk());
        assert_eq!(
            sample.containers,
            Some(ContainerCounts {
                total: 5,
                running: 1,
                stopped: 1,
                paused: 1,
                restarting: 1,
                other: 1,
                healthy: 1,
                unhealthy: 0,
                health_starting: 0,
                no_health: 2,
                unknown_health: 2,
            })
        );
        assert_eq!(sample.stacks, Some(1));
        assert_eq!(sample.images, Some(2));
        assert_eq!(sample.volumes, Some(1));
        assert_eq!(sample.networks, Some(0));
        assert_eq!(sample.version.unwrap().arch, "arm64");
        assert_eq!(disk.unwrap().images, Some(4096));
        let utilization = sample.utilization.unwrap();
        assert_eq!(utilization.running, 1);
        assert_eq!(utilization.usage.len(), 1);
        let usage = &utilization.usage[0];
        assert_eq!(usage.name, "web");
        assert_eq!(usage.cpu_percent, Some(40.));
        assert_eq!(usage.memory_bytes, Some(3072));
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }
}
