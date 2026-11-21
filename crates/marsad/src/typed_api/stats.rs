//! Stream bounded observations from the active guest control session.

use marsa_proto::{
    guest::v1::VmStats,
    host::v1::{VmStatsRequest, VmStatsSample, stats_server::Stats},
};
use tonic::{Request, Response, Status};

use super::{
    TypedApi,
    events::{self, Output},
};

impl TypedApi {
    /// Publish the latest stats received from the active, validated guest session.
    pub fn publish_guest_stats(&self, stats: VmStats) {
        publish(&self.inner.events, stats);
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
}
