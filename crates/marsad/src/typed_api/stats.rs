//! Stream bounded observations from the active guest control session.

use std::{io, time::Duration};

use marsa_proto::{
    guest::v1::VmStats,
    host::v1::{
        DockerOverviewRequest, DockerOverviewResponse, DockerOverviewSection, VmStatsRequest,
        VmStatsSample, stats_server::Stats,
    },
};
use tokio::time::{interval, timeout};
use tonic::{Request, Response, Status};

use crate::guest_control::docker_http;

use super::{
    TypedApi,
    events::{self, Output},
    inventory_cache::{self, InventoryCache},
};

impl TypedApi {
    /// Publish the latest stats received from the active, validated guest session.
    pub fn publish_guest_stats(&self, stats: VmStats) {
        publish(&self.inner.events, stats);
    }

    /// Refresh cacheable overview sections while the engine stays ready.
    pub(crate) fn spawn_inventory_refresh(&self) {
        let api = self.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(5));
            let mut closing = api.inner.closing.subscribe();
            loop {
                tokio::select! {
                    changed = closing.changed() => {
                        if changed.is_err() || *closing.borrow() {
                            break;
                        }
                    }
                    _ = tick.tick() => {}
                }
                if *closing.borrow() {
                    break;
                }
                if !matches!(
                    api.inner.events.engine_status().state(),
                    marsa_proto::host::v1::EngineState::Ready
                        | marsa_proto::host::v1::EngineState::Idle
                ) {
                    continue;
                }
                for section in InventoryCache::cacheable_sections() {
                    if *section == DockerOverviewSection::Disk
                        && let Ok(Some(cached)) = api.inner.inventory.get(*section)
                    {
                        let age = inventory_cache::now_millis()
                            .saturating_sub(cached.observed_at_unix_millis);
                        if age < 60_000 {
                            continue;
                        }
                    }
                    if let Err(error) = api.refresh_overview_section(*section).await {
                        tracing::debug!(?section, %error, "inventory cache refresh skipped");
                    }
                    if *closing.borrow() {
                        return;
                    }
                }
            }
        });
    }

    async fn refresh_overview_section(&self, section: DockerOverviewSection) -> Result<(), Status> {
        let request = DockerOverviewRequest {
            section: section.into(),
            container_id: String::new(),
            prefer_cache: false,
        };
        let _ = self.fetch_overview(request, false).await?;
        Ok(())
    }

    async fn fetch_overview(
        &self,
        request: DockerOverviewRequest,
        prefer_cache: bool,
    ) -> Result<DockerOverviewResponse, Status> {
        let section = request.section();
        if prefer_cache
            && InventoryCache::is_cacheable(section)
            && let Ok(Some(cached)) = self.inner.inventory.get(section)
        {
            return Ok(DockerOverviewResponse {
                json: cached.json,
                observed_at_unix_millis: cached.observed_at_unix_millis,
            });
        }

        let deadline = match section {
            DockerOverviewSection::Disk => Duration::from_secs(40),
            _ => Duration::from_secs(10),
        };
        let path = overview_path(request).map_err(Status::invalid_argument)?;
        let not_running = || Status::failed_precondition("Docker overview requires a running VM");
        let supervisor = self.supervisor.as_ref().ok_or_else(not_running)?;
        let json = timeout(deadline, async {
            let stream = supervisor.connect_running_docker().await?;
            docker_http::get(stream, &path, 2 * 1024 * 1024).await
        })
        .await
        .map_err(|_| Status::deadline_exceeded("Docker overview timed out"))?
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotConnected => not_running(),
            _ => Status::unavailable("could not read Docker overview"),
        })?;
        let observed_at_unix_millis = inventory_cache::now_millis();
        if InventoryCache::is_cacheable(section)
            && let Err(error) = self
                .inner
                .inventory
                .put(section, &json, observed_at_unix_millis)
        {
            tracing::warn!(%error, ?section, "failed to store overview section");
        }
        Ok(DockerOverviewResponse {
            json: json.to_vec(),
            observed_at_unix_millis,
        })
    }
}

pub(super) fn publish(bus: &events::Bus, stats: VmStats) {
    if stats.memory_used_bytes > stats.memory_total_bytes
        || stats.data_disk_used_bytes > stats.data_disk_total_bytes
    {
        return;
    }
    bus.stats(VmStatsSample {
        observed_at_unix_millis: events::now(),
        cpu_busy_nanos: stats.cpu_busy_nanos,
        memory_used_bytes: stats.memory_used_bytes,
        memory_total_bytes: stats.memory_total_bytes,
        disk_used_bytes: stats.data_disk_used_bytes,
        disk_total_bytes: stats.data_disk_total_bytes,
        network_rx_bytes: stats.network_rx_bytes,
        network_tx_bytes: stats.network_tx_bytes,
    });
}

#[tonic::async_trait]
impl Stats for TypedApi {
    type VmStatsStream = Output<VmStatsSample>;

    async fn vm_stats(
        &self,
        request: Request<VmStatsRequest>,
    ) -> Result<Response<Self::VmStatsStream>, Status> {
        self.authorize(request)?;
        Ok(Response::new(
            self.inner
                .events
                .watch_stats(self.inner.closing.subscribe()),
        ))
    }

    async fn docker_overview(
        &self,
        request: Request<DockerOverviewRequest>,
    ) -> Result<Response<DockerOverviewResponse>, Status> {
        let request = self.authorize(request)?;
        let prefer_cache = request.prefer_cache;
        Ok(Response::new(
            self.fetch_overview(request, prefer_cache).await?,
        ))
    }
}

fn overview_path(request: DockerOverviewRequest) -> Result<String, &'static str> {
    let section = request.section();
    let id = &request.container_id;
    if section == DockerOverviewSection::ContainerStats {
        if id.len() != 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid Docker container identifier");
        }
        return Ok(format!("/v1.57/containers/{id}/stats?stream=false"));
    }
    if !id.is_empty() {
        return Err("unexpected Docker container identifier");
    }
    Ok(match section {
        DockerOverviewSection::Containers => "/v1.57/containers/json?all=true",
        DockerOverviewSection::Images => "/v1.57/images/json?all=true",
        DockerOverviewSection::Volumes => "/v1.57/volumes",
        DockerOverviewSection::Networks => "/v1.57/networks",
        DockerOverviewSection::Disk => "/v1.57/system/df?verbose=true",
        DockerOverviewSection::Version => "/v1.57/version",
        _ => return Err("invalid Docker overview section"),
    }
    .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_routes_accept_only_fixed_queries_and_full_container_ids() {
        let make = |section: DockerOverviewSection, id: &str| DockerOverviewRequest {
            section: section.into(),
            container_id: id.into(),
            prefer_cache: false,
        };
        assert_eq!(
            overview_path(make(DockerOverviewSection::Disk, "")).unwrap(),
            "/v1.57/system/df?verbose=true"
        );
        let id = "1a".repeat(32);
        assert_eq!(
            overview_path(make(DockerOverviewSection::ContainerStats, &id)).unwrap(),
            format!("/v1.57/containers/{id}/stats?stream=false")
        );
        assert!(overview_path(make(DockerOverviewSection::ContainerStats, "../json")).is_err());
        assert!(overview_path(make(DockerOverviewSection::Containers, &id)).is_err());
        assert!(overview_path(make(DockerOverviewSection::Unspecified, "")).is_err());
    }
}
