//! Produce a bounded support archive from explicitly approved diagnostic fields.

mod logs;
mod storage;
mod summaries;

use std::{collections::VecDeque, io};

use marsa_core::settings::{NetworkMode, Settings};
use marsa_proto::{
    guest::PROTOCOL_VERSION,
    host::{
        API_MAJOR, API_MINOR,
        v1::{
            CollectDiagnosticsRequest, CollectDiagnosticsResponse, EngineHealthState, EngineState,
            EngineStatus, LogEntry, LogsRequest, NetworkStatus, diagnostics_server::Diagnostics,
        },
    },
};
use serde_json::{Value, json};
use tonic::{Request, Response, Status};

use super::{TypedApi, events::Output};

#[tonic::async_trait]
impl Diagnostics for TypedApi {
    type LogsStream = Output<LogEntry>;

    async fn collect(
        &self,
        request: Request<CollectDiagnosticsRequest>,
    ) -> Result<Response<CollectDiagnosticsResponse>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let response = self
            .inner
            .diagnostics_requests
            .run(
                &request.request_id,
                request.include_agent_audit,
                || async move {
                    if request.include_agent_audit {
                        return Err(Status::unimplemented(
                            "agent audit collection is unavailable",
                        ));
                    }
                    let snapshot = api.inner.events.diagnostic_snapshot();
                    let path = api.inner.paths.clone();
                    let uid = api.inner.uid;
                    let bundle_path =
                        tokio::task::spawn_blocking(move || storage::collect(&path, uid, snapshot))
                            .await
                            .map_err(|_| Status::internal("diagnostic archive worker stopped"))?
                            .map_err(collection_error)?;
                    Ok(CollectDiagnosticsResponse { bundle_path })
                },
            )
            .await?;
        Ok(Response::new(response))
    }

    async fn logs(
        &self,
        request: Request<LogsRequest>,
    ) -> Result<Response<Self::LogsStream>, Status> {
        let request = self.authorize(request)?;
        logs::stream(
            self.inner.paths.clone(),
            self.inner.uid,
            self.inner.closing.subscribe(),
            request,
        )
        .map(Response::new)
    }
}

fn collection_error(error: io::Error) -> Status {
    if error.kind() == io::ErrorKind::PermissionDenied {
        Status::permission_denied("the diagnostic output directory is not private")
    } else {
        tracing::warn!(kind = ?error.kind(), "could not collect diagnostics");
        Status::internal("could not create the diagnostic archive")
    }
}

pub(super) struct EngineSample {
    sequence: u64,
    observed_ms: i64,
    state: EngineState,
    health: EngineHealthState,
    stage: Option<&'static str>,
}

impl EngineSample {
    pub(super) fn new(status: &EngineStatus, sequence: u64, observed_ms: i64) -> Self {
        Self {
            sequence,
            observed_ms,
            state: status.state(),
            health: status.guest_health(),
            stage: [
                "prepare",
                "create",
                "start",
                "handshake",
                "network",
                "engine",
                "running",
                "resume",
                "shutdown",
            ]
            .into_iter()
            .find(|stage| Some(*stage) == status.boot_stage.as_deref()),
        }
    }

    fn json(&self) -> Value {
        json!({
            "sequence": self.sequence,
            "observed_at_unix_ms": self.observed_ms,
            "state": self.state.as_str_name(),
            "health": self.health.as_str_name(),
            "failed_stage": self.stage,
        })
    }
}

pub(super) struct Snapshot {
    settings: Value,
    state: Value,
    network: Value,
}

impl Snapshot {
    pub(super) fn new(
        settings: &Settings,
        engine: &EngineStatus,
        history: &VecDeque<EngineSample>,
        network: Option<&NetworkStatus>,
        restart_required: bool,
    ) -> Self {
        let latest = history
            .back()
            .map(|sample| (sample.sequence, sample.observed_ms));
        let (sequence, time) = latest.unwrap_or((0, super::events::now()));
        let backend = ["vz", "krun", "fake"]
            .into_iter()
            .find(|value| *value == engine.backend);
        Self {
            settings: json!({
                "cpus": settings.vm.cpus,
                "memory_bytes": settings.vm.memory,
                "configured_backend": super::events::backend_name(settings.vm.backend),
                "idle_stop": settings.vm.idle_stop,
                "idle_timeout_seconds": settings.vm.idle_timeout,
                "data_size_limit_bytes": settings.disk.data_size_limit,
                "auto_reclaim": settings.disk.auto_reclaim,
                "share_count": settings.shares.len(),
                "manual_dns_server_count": settings.network.dns.len(),
                "http_proxy_override": settings.network.proxy.http.is_some(),
                "https_proxy_override": settings.network.proxy.https.is_some(),
                "inject_proxy_into_containers": settings.network.proxy.inject_into_containers,
                "restart_required": restart_required,
            }),
            state: json!({
                "current": EngineSample::new(engine, sequence, time).json(),
                "backend": backend,
                "history": history.iter().map(EngineSample::json).collect::<Vec<_>>(),
            }),
            network: json!({
                "configured_mode": match settings.network.mode {
                    NetworkMode::Auto => "auto",
                    NetworkMode::Nat => "nat",
                    NetworkMode::Userspace => "userspace",
                },
                "active_mode": network.map(|network| network.active_mode().as_str_name()),
                "boot_result": network.and_then(|network| summaries::network_reason(network.reason.as_deref())),
            }),
        }
    }
}

fn versions() -> Value {
    json!({
        "bundle_schema": 1,
        "daemon": env!("CARGO_PKG_VERSION"),
        "host_os": std::env::consts::OS,
        "host_arch": std::env::consts::ARCH,
        "host_api": { "major": API_MAJOR, "minor": API_MINOR },
        "guest_control": { "major": PROTOCOL_VERSION.major, "minor": PROTOCOL_VERSION.minor },
    })
}
