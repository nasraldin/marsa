//! Read-only dashboard projections scoped to one Docker engine.

use std::{collections::BTreeSet, time::Duration};

use marsa_proto::host::{
    DOCKER_INVENTORY_RESPONSE_LIMIT,
    docker_registry::{
        DistributionInspect, REGISTRY_DIGEST_API_MINOR, REGISTRY_DIGEST_TIMEOUT,
        valid_registry_digest,
    },
    v1::{DockerOverviewRequest, DockerOverviewSection as Section, DockerRegistryDigestRequest},
};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::{task::JoinSet, time::sleep};

use crate::{ClientError, MarsaClient};

use super::{
    ContainerId, ContainerSummary, DockerClient, DockerError, ExternalEndpoint, ImageSummary,
    NetworkSummary, StatsSample, VolumeSummary, environment_report::display_value,
};

const OVERVIEW_API_MINOR: u32 = 13;
/// Host API minor that serves `prefer_cache` overview reads (D-034).
const CACHE_API_MINOR: u32 = 14;
const SAMPLE_LIMIT: usize = 48;
const SAMPLE_CONCURRENCY: usize = 8;
const SAMPLE_BUDGET: Duration = Duration::from_secs(8);
pub(super) const DISK_TIMEOUT: Duration = Duration::from_secs(40);
pub(super) const DISK_PATH: &str = "/system/df?verbose=true";
pub(super) const VERSION_PATH: &str = "/version";
pub(super) const STATS_PATH: &str = "stats?stream=false";

fn section_timeout(section: Section) -> Duration {
    if section == Section::Disk {
        DISK_TIMEOUT
    } else {
        super::REQUEST_TIMEOUT
    }
}

impl MarsaClient {
    pub(super) async fn read_docker_snapshot<T: DeserializeOwned>(
        &self,
        section: Section,
        container: Option<&ContainerId>,
        prefer_cache: bool,
    ) -> Result<T, DockerError> {
        let prefer_cache = prefer_cache && self.api_minor() >= CACHE_API_MINOR;
        let mut request = tonic::Request::new(DockerOverviewRequest {
            section: section.into(),
            container_id: container.map_or_else(String::new, |id| id.as_str().to_owned()),
            prefer_cache,
            activity_hours: None,
        });
        request.set_timeout(if prefer_cache {
            Duration::from_secs(2)
        } else {
            section_timeout(section)
        });
        let response = self
            .stats()
            .max_decoding_message_size(DOCKER_INVENTORY_RESPONSE_LIMIT)
            .docker_overview(request)
            .await
            .map_err(ClientError::from)?
            .into_inner();
        Ok(serde_json::from_slice(&response.json)?)
    }
}

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
        self.sample_with_image_checks(true).await
    }

    /// Read live observations, optionally comparing installed image tags with registry digests.
    pub async fn sample_with_image_checks(&self, check: bool) -> OverviewSample {
        self.sample_with_cache(false, check).await
    }

    /// Prefer the daemon inventory cache for a fast first frame (Marsa only).
    pub async fn sample_cached(&self) -> OverviewSample {
        self.sample_with_cache(true, false).await
    }

    async fn sample_with_cache(
        &self,
        prefer_cache: bool,
        check_image_updates: bool,
    ) -> OverviewSample {
        let containers = async {
            let rows: Vec<ContainerSummary> =
                self.read(Section::Containers, None, prefer_cache).await?;
            let stacks = rows
                .iter()
                .filter_map(ContainerSummary::compose_project)
                .collect::<BTreeSet<_>>()
                .len();
            let utilization = if prefer_cache {
                OverviewUtilization {
                    running: rows.iter().filter(|row| row.state == "running").count(),
                    usage: Vec::new(),
                }
            } else {
                self.container_utilization(&rows).await
            };
            Some((rows, stacks, utilization))
        };
        let images = self.read::<Vec<ImageSummary>>(Section::Images, None, prefer_cache);
        let networks = self.read::<Vec<NetworkSummary>>(Section::Networks, None, prefer_cache);
        let volumes = self.read::<Volumes>(Section::Volumes, None, prefer_cache);
        let version = self.read::<EngineVersion>(Section::Version, None, prefer_cache);
        let (containers, images, volumes, networks, version) =
            tokio::join!(containers, images, volumes, networks, version);
        let updates_checked = check_image_updates && containers.is_some() && images.is_some();
        let (containers, stacks, utilization, outdated_images) = match containers {
            Some((rows, stacks, utilization)) => {
                let (updates, outdated_images) =
                    if let Some(images) = images.as_ref().filter(|_| check_image_updates) {
                        match self {
                            Self::External(_) => self.registry_image_updates(&rows, images).await,
                            Self::Marsa(_) => local_image_updates(&rows, images),
                        }
                    } else {
                        (0, BTreeSet::new())
                    };
                let mut counts = ContainerCounts::from_rows(&rows);
                counts.updates = updates;
                (
                    Some(counts),
                    Some(stacks),
                    Some(utilization),
                    outdated_images,
                )
            }
            None => (None, None, None, BTreeSet::new()),
        };
        OverviewSample {
            containers,
            stacks,
            images: images.map(|rows| rows.len()),
            volumes: volumes.map(|result| result.volumes.len()),
            networks: networks.map(|rows| rows.len()),
            version: version.and_then(EngineVersion::checked),
            utilization,
            updates_checked,
            outdated_images,
        }
    }

    /// Read engine identity separately from inventory and live measurements.
    pub async fn engine_version(&self, prefer_cache: bool) -> Option<EngineVersion> {
        self.read::<EngineVersion>(Section::Version, None, prefer_cache)
            .await
            .and_then(EngineVersion::checked)
    }

    /// Measure storage separately because the engine may need to scan its filesystem.
    pub async fn disk(&self) -> Option<DiskBreakdown> {
        self.read::<EngineDiskUsage>(Section::Disk, None, false)
            .await
            .map(EngineDiskUsage::project)
            .filter(DiskBreakdown::has_measurements)
    }

    /// Prefer a cached disk breakdown when the daemon has one (Marsa only).
    pub async fn disk_cached(&self) -> Option<DiskBreakdown> {
        self.read::<EngineDiskUsage>(Section::Disk, None, true)
            .await
            .map(EngineDiskUsage::project)
            .filter(DiskBreakdown::has_measurements)
    }

    /// Compare this exact inventory against the registry without reloading resource collections.
    pub async fn registry_image_updates(
        &self,
        containers: &[ContainerSummary],
        images: &[ImageSummary],
    ) -> (usize, BTreeSet<String>) {
        let remotes = self.remote_tag_digests(containers).await;
        (
            outdated_updates(containers, images, &remotes),
            outdated_image_refs(containers, images, &remotes),
        )
    }

    /// Resolve registry digests for the distinct tagged references used by containers.
    async fn remote_tag_digests(
        &self,
        containers: &[ContainerSummary],
    ) -> std::collections::BTreeMap<String, String> {
        let mut unique = BTreeSet::new();
        for container in containers {
            if let Some(reference) = normalize_image_ref(&container.image) {
                unique.insert(reference);
            }
        }
        // Prefer namespaced refs (user/org images) so Hub lookups like dockhand finish
        // before slow 403s on local-only names burn the budget.
        let mut pending: Vec<_> = unique.into_iter().collect();
        pending.sort_by(|left, right| {
            registry_probe_priority(left)
                .cmp(&registry_probe_priority(right))
                .then_with(|| left.cmp(right))
        });
        if matches!(self, Self::Marsa(_)) {
            // Bound each opt-in cycle independently of the number of installed workloads.
            pending.truncate(32);
        }
        let mut pending = pending.into_iter();
        let mut digests = std::collections::BTreeMap::new();
        let mut jobs = JoinSet::new();
        let deadline = sleep(Duration::from_secs(20));
        tokio::pin!(deadline);
        loop {
            while jobs.len() < 4 {
                let Some(reference) = pending.next() else {
                    break;
                };
                let client = self.clone();
                jobs.spawn(async move {
                    let digest = client.distribution_digest(&reference).await;
                    (reference, digest)
                });
            }
            if jobs.is_empty() {
                break;
            }
            tokio::select! {
                result = jobs.join_next() => {
                    if let Some(Ok((reference, Some(digest)))) = result {
                        digests.insert(reference, digest);
                    }
                }
                () = &mut deadline => {
                    // Keep digests already finished; drop the rest so Overview stays responsive.
                    while let Some(result) = jobs.try_join_next() {
                        if let Ok((reference, Some(digest))) = result {
                            digests.insert(reference, digest);
                        }
                    }
                    break;
                }
            }
        }
        digests
    }

    async fn distribution_digest(&self, reference: &str) -> Option<String> {
        match self {
            Self::External(client) => {
                let path = format!("/distribution/{reference}/json");
                client
                    .snapshot_with_timeout::<DistributionInspect>(&path, Duration::from_secs(5))
                    .await
                    .ok()?
                    .into_digest()
            }
            Self::Marsa(client) if client.api_minor() >= REGISTRY_DIGEST_API_MINOR => {
                let mut request = tonic::Request::new(DockerRegistryDigestRequest {
                    reference: reference.to_owned(),
                });
                request.set_timeout(REGISTRY_DIGEST_TIMEOUT + Duration::from_secs(1));
                let response = client
                    .stats()
                    .max_decoding_message_size(512)
                    .docker_registry_digest(request)
                    .await
                    .ok()?
                    .into_inner();
                valid_registry_digest(&response.digest).then_some(response.digest)
            }
            Self::Marsa(_) => None,
        }
    }

    /// Read live measurements for running containers from an already-observed listing.
    pub async fn container_utilization<'a>(
        &self,
        rows: impl IntoIterator<Item = &'a ContainerSummary, IntoIter: Clone + Send> + Send,
    ) -> OverviewUtilization {
        let mut rows = rows.into_iter().filter(|row| row.state == "running");
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
                let stats: StatsSample = client
                    .read(Section::ContainerStats, Some(&id), false)
                    .await?;
                let cpu_percent = stats.cpu_percent();
                let memory_bytes = stats.memory_stats.used_without_cache();
                let memory_limit = stats.memory_stats.limit.filter(|limit| *limit > 0);
                let online_cpus = stats
                    .cpu_stats
                    .online_cpus
                    .filter(|cpus| *cpus > 0)
                    .or_else(|| {
                        u32::try_from(stats.cpu_stats.cpu_usage.percpu_usage.len())
                            .ok()
                            .filter(|cpus| *cpus > 0)
                    });
                (cpu_percent.is_some() || memory_bytes.is_some()).then_some(ContainerUtilization {
                    id,
                    name,
                    cpu_percent,
                    memory_bytes,
                    memory_limit,
                    online_cpus,
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
        prefer_cache: bool,
    ) -> Option<T> {
        match self {
            Self::Marsa(client) if client.api_minor() >= OVERVIEW_API_MINOR => client
                .read_docker_snapshot(section, container, prefer_cache)
                .await
                .ok(),
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
                    Section::Activity | Section::Unspecified => return None,
                };
                client
                    .snapshot_with_timeout(&path, section_timeout(section))
                    .await
                    .ok()
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
#[derive(Clone, Debug, Default)]
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
    /// Registry image checks were enabled and inventory was available for this sample.
    pub updates_checked: bool,
    /// Normalized image refs with a newer local tag or registry digest.
    pub outdated_images: BTreeSet<String>,
}

/// Counts of lifecycle and health states from a single listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// Containers whose image tag now points at a newer digest.
    pub updates: usize,
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
    /// Count lifecycle and health states in an observed container listing.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = &'a ContainerSummary>) -> Self {
        let mut counts = Self::default();
        for row in rows {
            counts.total += 1;
            match row.state.as_str() {
                "running" => counts.running += 1,
                "created" | "exited" | "dead" => counts.stopped += 1,
                "paused" => counts.paused += 1,
                "restarting" => counts.restarting += 1,
                _ => counts.other += 1,
            }
            match row.health() {
                Some("healthy") => counts.healthy += 1,
                Some("unhealthy") => counts.unhealthy += 1,
                Some("starting") => counts.health_starting += 1,
                Some("none") => counts.no_health += 1,
                _ => counts.unknown_health += 1,
            }
        }
        counts
    }
}

/// Count containers whose named tag is newer locally or on the registry.
pub fn outdated_updates(
    containers: &[ContainerSummary],
    images: &[ImageSummary],
    remotes: &std::collections::BTreeMap<String, String>,
) -> usize {
    let tags = image_tag_index(images);
    containers
        .iter()
        .filter(|container| container_needs_update(container, images, &tags, remotes))
        .count()
}

/// Compare installed tags against container image IDs without making engine requests.
pub fn local_image_updates(
    containers: &[ContainerSummary],
    images: &[ImageSummary],
) -> (usize, BTreeSet<String>) {
    let tags = image_tag_index(images);
    let remotes = std::collections::BTreeMap::new();
    let outdated = containers
        .iter()
        .filter(|container| container_needs_update(container, images, &tags, &remotes));
    (
        outdated.clone().count(),
        outdated
            .filter_map(|container| normalize_image_ref(&container.image))
            .collect(),
    )
}

/// Lower values are probed first when resolving registry digests.
fn registry_probe_priority(reference: &str) -> u8 {
    let name = reference
        .rsplit_once(':')
        .map_or(reference, |(name, _)| name);
    if name.contains('.') || name.starts_with('[') {
        0
    } else if name.contains('/') {
        1
    } else if name.ends_with(".local") || reference.ends_with(":local") {
        3
    } else {
        2
    }
}

/// Normalized image references that currently need an update.
pub fn outdated_image_refs(
    containers: &[ContainerSummary],
    images: &[ImageSummary],
    remotes: &std::collections::BTreeMap<String, String>,
) -> BTreeSet<String> {
    let tags = image_tag_index(images);
    containers
        .iter()
        .filter(|container| container_needs_update(container, images, &tags, remotes))
        .filter_map(|container| normalize_image_ref(&container.image))
        .collect()
}

#[cfg(test)]
mod outdated_tests {
    use super::*;

    #[test]
    fn counts_containers_whose_tag_moved() {
        let containers: Vec<ContainerSummary> = serde_json::from_value(serde_json::json!([
            {
                "Id": "a".repeat(64),
                "Names": ["/old"],
                "Image": "nginx:latest",
                "ImageID": "sha256:olddigest",
                "Created": 0,
                "State": "running",
                "Status": "Up",
            },
            {
                "Id": "b".repeat(64),
                "Names": ["/fresh"],
                "Image": "nginx:latest",
                "ImageID": "sha256:newdigest",
                "Created": 0,
                "State": "running",
                "Status": "Up",
            },
            {
                "Id": "c".repeat(64),
                "Names": ["/pin"],
                "Image": "sha256:deadbeef",
                "ImageID": "sha256:deadbeef",
                "Created": 0,
                "State": "exited",
                "Status": "Exited",
            }
        ]))
        .unwrap();
        let images: Vec<ImageSummary> = serde_json::from_value(serde_json::json!([
            {
                "Id": "sha256:newdigest",
                "RepoTags": ["nginx:latest"],
                "RepoDigests": ["nginx@sha256:newdigest"],
                "Created": 1,
                "Size": 1
            }
        ]))
        .unwrap();
        assert_eq!(
            outdated_updates(&containers, &images, &Default::default()),
            1
        );
        assert_eq!(
            local_image_updates(&containers, &images),
            (1, BTreeSet::from(["nginx:latest".to_owned()]))
        );
    }

    #[test]
    fn counts_containers_behind_registry_digest() {
        let containers: Vec<ContainerSummary> = serde_json::from_value(serde_json::json!([{
            "Id": "a".repeat(64),
            "Names": ["/ui-dockhand"],
            "Image": "fnsys/dockhand:latest",
            "ImageID": "sha256:4fd64d32728c99e5caf39b952a17d85448ee0b8ae76e0961f719ba2e19013988",
            "Created": 0,
            "State": "running",
            "Status": "Up",
        }]))
        .unwrap();
        let images: Vec<ImageSummary> = serde_json::from_value(serde_json::json!([{
            "Id": "sha256:4fd64d32728c99e5caf39b952a17d85448ee0b8ae76e0961f719ba2e19013988",
            "RepoTags": ["fnsys/dockhand:latest"],
            "RepoDigests": ["fnsys/dockhand@sha256:4fd64d32728c99e5caf39b952a17d85448ee0b8ae76e0961f719ba2e19013988"],
            "Created": 1,
            "Size": 1
        }]))
        .unwrap();
        let remotes = std::collections::BTreeMap::from([(
            "fnsys/dockhand:latest".into(),
            "sha256:c2b0fc55e3ed299133f6caf63a764deb118ee402ea761548c3f8fd01d04fca06".into(),
        )]);
        assert_eq!(outdated_updates(&containers, &images, &remotes), 1);
        assert_eq!(
            outdated_updates(&containers, &images, &Default::default()),
            0
        );
    }

    #[test]
    fn normalizes_pinned_tag_digest_refs() {
        assert_eq!(
            normalize_image_ref(
                "postgres:18.6-trixie@sha256:e3d24f6906d0760bf4da0dae872415391ededefad23fd471c47dc53b0ddf3c42"
            )
            .as_deref(),
            Some("postgres:18.6-trixie")
        );
        assert_eq!(
            normalize_image_ref("deploy-web").as_deref(),
            Some("deploy-web:latest")
        );
        assert_eq!(normalize_image_ref("sha256:deadbeef"), None);
    }

    #[test]
    fn digests_pins_never_count_as_updates() {
        let containers: Vec<ContainerSummary> = serde_json::from_value(serde_json::json!([{
            "Id": "a".repeat(64),
            "Names": ["/pin"],
            "Image": "postgres:18.6@sha256:edb0c08dc33befd0262ab3b5cf1c4f3f8cfd8b09e88c05b45adee29d6761ddb6",
            "ImageID": "sha256:edb0c08dc33befd0262ab3b5cf1c4f3f8cfd8b09e88c05b45adee29d6761ddb6",
            "Created": 0,
            "State": "running",
            "Status": "Up",
        }]))
        .unwrap();
        let remotes = std::collections::BTreeMap::from([(
            "postgres:18.6".into(),
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
        )]);
        assert_eq!(outdated_updates(&containers, &[], &remotes), 0);
    }
}

/// Whether this container should show under Updates.
pub fn container_needs_update(
    container: &ContainerSummary,
    images: &[ImageSummary],
    tags: &std::collections::BTreeMap<&str, &str>,
    remotes: &std::collections::BTreeMap<String, String>,
) -> bool {
    // Digest pins are immutable; only floating tags can be outdated.
    if container.image.contains("@sha256:") {
        return false;
    }
    let Some(reference) = normalize_image_ref(&container.image) else {
        return false;
    };
    if let Some(latest) = tags.get(reference.as_str())
        && !image_ids_match(latest, &container.image_id)
    {
        return true;
    }
    let Some(remote) = remotes.get(&reference) else {
        return false;
    };
    if image_ids_match(remote, &container.image_id) {
        return false;
    }
    !image_knows_digest(images, &container.image_id, remote)
}

/// Map each repo tag to the image id that currently owns it.
pub fn image_tag_index(images: &[ImageSummary]) -> std::collections::BTreeMap<&str, &str> {
    let mut tags = std::collections::BTreeMap::new();
    for image in images {
        for tag in &image.repo_tags {
            if tag.is_empty() || tag == "<none>:<none>" {
                continue;
            }
            tags.insert(tag.as_str(), image.id.as_str());
        }
    }
    tags
}

/// Strip digest pins and apply Docker's implicit `:latest` rule.
pub fn normalize_image_ref(image: &str) -> Option<String> {
    let image = image.trim();
    if image.is_empty() {
        return None;
    }
    // Image IDs and digest-only refs are immutable pins, not updateable tags.
    if let Some(digest) = image.strip_prefix("sha256:")
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let without_digest = match image.split_once("@sha256:") {
        Some((name, digest))
            if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            name
        }
        Some(_) => return None,
        None => image,
    };
    if without_digest.is_empty() {
        return None;
    }
    let name = without_digest
        .rsplit_once('/')
        .map_or(without_digest, |(_, name)| name);
    if name.contains(':') {
        Some(without_digest.to_owned())
    } else {
        Some(format!("{without_digest}:latest"))
    }
}

fn image_knows_digest(images: &[ImageSummary], image_id: &str, digest: &str) -> bool {
    images.iter().any(|image| {
        if !image_ids_match(&image.id, image_id) {
            return false;
        }
        image_ids_match(&image.id, digest)
            || image.repo_digests.iter().any(|entry| {
                let value = entry
                    .rsplit_once('@')
                    .map_or(entry.as_str(), |(_, digest)| digest);
                image_ids_match(value, digest)
            })
    })
}

fn image_ids_match(left: &str, right: &str) -> bool {
    let left = left.strip_prefix("sha256:").unwrap_or(left);
    let right = right.strip_prefix("sha256:").unwrap_or(right);
    left == right || left.starts_with(right) || right.starts_with(left)
}

/// Sample coverage for the currently running containers.
#[derive(Clone, Debug)]
pub struct OverviewUtilization {
    /// Number of containers eligible for sampling.
    pub running: usize,
    /// Containers with at least one measurement, in no particular order.
    pub usage: Vec<ContainerUtilization>,
}

/// Measured resource use reported by Docker for one container.
#[derive(Clone, Debug)]
pub struct ContainerUtilization {
    /// Full container ID, used to break ties reproducibly.
    pub id: ContainerId,
    /// Engine name or the short ID when no name is present.
    pub name: String,
    /// Percentage of one logical CPU, if the engine supplied both counters.
    pub cpu_percent: Option<f64>,
    /// Memory in bytes excluding inactive file cache, where reported.
    pub memory_bytes: Option<u64>,
    /// Memory cgroup limit in bytes, typically the host budget for the engine.
    pub memory_limit: Option<u64>,
    /// Online logical CPUs reported with the sample.
    pub online_cpus: Option<u32>,
}

impl OverviewUtilization {
    /// Strongest memory limit reported with the sampled containers.
    pub fn memory_limit(&self) -> Option<u64> {
        self.usage.iter().filter_map(|row| row.memory_limit).max()
    }

    /// Online CPU count shared by the sampled containers.
    pub fn online_cpus(&self) -> Option<u32> {
        self.usage.iter().filter_map(|row| row.online_cpus).max()
    }
}

/// Docker Engine version and platform.
#[derive(Clone, Debug, Deserialize)]
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
        let healthy = container("a", "running", "Up 2 minutes (healthy)", None);
        let reported: ContainerSummary = serde_json::from_value(healthy.clone()).unwrap();
        assert_eq!(reported.health(), Some("healthy"));
        let inventory = serde_json::to_string(&[
            healthy,
            container("b", "paused", "Up 1 hour (Paused)", Some("none")),
            container("c", "restarting", "Restarting", Some("none")),
            container("d", "exited", "Exited", None),
            container("e", "removing", "", None),
        ])
        .unwrap();
        let routes = std::collections::BTreeMap::from([
            ("/containers/json?all=true".into(), inventory),
            (
                "/images/json?all=true".into(),
                r#"[{"Id":"sha256:l2m","RepoTags":["example/web:latest"],"RepoDigests":["example/web@sha256:l2m"],"Created":0,"Size":128},{"Id":"sha256:second","Created":0,"Size":256}]"#.into(),
            ),
            ("/volumes".into(), r#"{"Volumes":[{"Name":"web-data","Driver":"local","Scope":"local"}]}"#.into()),
            ("/networks".into(), "[]".into()),
            (VERSION_PATH.into(), r#"{"Version":"29.10.0","Os":"linux","Arch":"arm64"}"#.into()),
            (DISK_PATH.into(), r#"{"ImageUsage":{"TotalSize":4096},"ContainerUsage":{},"VolumeUsage":{"TotalSize":512},"BuildCacheUsage":{"TotalSize":1024}}"#.into()),
            (stats_path, r#"{"memory_stats":{"usage":4096,"stats":{"inactive_file":1024}},"cpu_stats":{"system_cpu_usage":130,"cpu_usage":{"total_usage":18},"online_cpus":2},"precpu_stats":{"system_cpu_usage":100,"cpu_usage":{"total_usage":12}}}"#.into()),
            (
                "/distribution/example/web:latest/json".into(),
                r#"{"Descriptor":{"mediaType":"application/vnd.oci.image.index.v1+json","digest":"sha256:c2b0fc55e3ed299133f6caf63a764deb118ee402ea761548c3f8fd01d04fca06","size":1609}}"#.into(),
            ),
        ]);
        let distribution_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_requests = distribution_requests.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let server = tokio::spawn(async move {
            let mut replies = JoinSet::new();
            let routes = std::sync::Arc::new(routes);
            // Inventory + stats + disk + distribution can overlap; serve until clients disconnect.
            for _ in 0..16 {
                let Ok(Ok((mut socket, _))) =
                    tokio::time::timeout(Duration::from_millis(500), listener.accept()).await
                else {
                    break;
                };
                let routes = routes.clone();
                let observed_requests = observed_requests.clone();
                let sent = sent.clone();
                replies.spawn(async move {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.lines().next().unwrap().strip_prefix("GET ").unwrap().strip_suffix(" HTTP/1.1").unwrap();
                    sent.send(path.to_owned()).unwrap();
                    if path.starts_with("/distribution/") {
                        observed_requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let body = routes.get(path).map(String::as_str).unwrap_or("{}");
                    let status = if routes.contains_key(path) {
                        "200 OK"
                    } else {
                        "404 Not Found"
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
                updates: 5,
                healthy: 1,
                unhealthy: 0,
                health_starting: 0,
                no_health: 2,
                unknown_health: 2,
            })
        );
        assert_eq!(
            sample.outdated_images,
            BTreeSet::from(["example/web:latest".into()])
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
        assert!(sample.updates_checked);
        let without_checks = client.sample_with_image_checks(false).await;
        assert!(without_checks.containers.is_some());
        assert!(!without_checks.updates_checked);
        assert!(without_checks.outdated_images.is_empty());
        assert_eq!(
            distribution_requests.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        received.try_iter().for_each(drop);
        for _ in 0..2 {
            let repeated = client.container_utilization([&reported]).await;
            assert_eq!(repeated.running, 1);
            assert_eq!(repeated.usage[0].memory_bytes, Some(3072));
        }
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            vec![format!("/containers/{}/stats?stream=false", "a".repeat(64)); 2]
        );
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }
}
