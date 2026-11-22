//! Capture state and subscribe at one cursor under the same lock as publishers.

use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    time::SystemTime,
};

use futures_util::{Stream, stream};
use marsa_core::settings::{NetworkMode, Settings, VmBackend};
use marsa_proto::guest::v1::EgressStage;
use marsa_proto::host::v1::{
    DaemonEvent, DaemonSnapshot, EngineHealthState, EngineStatus, NetworkMode as WireMode,
    NetworkStatus, SettingsChanged, SettingsSnapshot, SubscribeRequest, VmStatsSample,
    daemon_event::Body, events_server::Events,
};
use marsa_vm::NetworkSpec;
use tokio::sync::{broadcast, watch};
use tonic::{Request, Response, Status};

use crate::supervisor::{AlternateFailure, BootNetwork, NetworkResult, State, Transition};

use super::{TypedApi, diagnostics, engine};

const CAPACITY: usize = 128;
pub(super) type Output<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[derive(Clone)]
pub(super) struct Bus(Arc<Mutex<StateAtCursor>>);

struct StateAtCursor {
    stream_id: String,
    sequence: u64,
    engine: EngineStatus,
    network: Option<NetworkStatus>,
    startup: Settings,
    settings: Settings,
    settings_snapshot: SettingsSnapshot,
    stats: Option<VmStatsSample>,
    engine_history: VecDeque<diagnostics::EngineSample>,
    events: broadcast::Sender<DaemonEvent>,
    settings_events: broadcast::Sender<SettingsSnapshot>,
    stats_events: broadcast::Sender<VmStatsSample>,
}

impl Bus {
    pub(super) fn new(settings: Settings) -> io::Result<Self> {
        let mut stream_id = [0; 16];
        getrandom::fill(&mut stream_id).map_err(io::Error::other)?;
        let mut initial_revision = [0; 8];
        initial_revision.copy_from_slice(&stream_id[..8]);
        let initial_revision = (u64::from_be_bytes(initial_revision) >> 1) + 1;
        let initial_toml = settings.to_toml().map_err(io::Error::other)?;
        Ok(Self(Arc::new(Mutex::new(StateAtCursor {
            stream_id: stream_id.iter().map(|byte| format!("{byte:02x}")).collect(),
            sequence: 0,
            engine: EngineStatus {
                state: engine::wire_state(State::Stopped).into(),
                backend: backend_name(settings.vm.backend).into(),
                ..Default::default()
            },
            network: None,
            startup: settings.clone(),
            settings,
            settings_snapshot: SettingsSnapshot {
                settings_toml: initial_toml,
                revision: initial_revision,
                restart_required: false,
            },
            stats: None,
            engine_history: VecDeque::new(),
            events: broadcast::channel(CAPACITY).0,
            settings_events: broadcast::channel(8).0,
            stats_events: broadcast::channel(CAPACITY).0,
        }))))
    }

    pub(super) fn backend(&self, name: &str) {
        self.lock().engine.backend = name.into();
    }

    pub(super) fn configured_network_mode(&self) -> NetworkMode {
        self.lock().settings.network.mode
    }

    pub(super) fn engine_status(&self) -> EngineStatus {
        self.lock().engine.clone()
    }

    pub(super) fn engine(&self, transition: &Transition) {
        let mut state = self.lock();
        let health = match transition.current {
            State::Stopped | State::Starting | State::Stopping | State::Failed => {
                state.stats = None;
                EngineHealthState::Unspecified.into()
            }
            _ => state.engine.guest_health,
        };
        state.engine = engine::from_transition(&state.engine.backend, transition, health);
        let status = state.engine.clone();
        state.publish(Body::Engine(status));

        let previous = state.network.take();
        let configured = state.startup.network.mode;
        let current = transition
            .network
            .map(|network| network_status(configured, network));
        if current != previous {
            let status = current.clone().unwrap_or_else(|| NetworkStatus {
                configured_mode: mode(configured).into(),
                ..Default::default()
            });
            state.publish(Body::Network(status));
        }
        state.network = current;
    }

    pub(super) fn health(&self, health: EngineHealthState) {
        let mut state = self.lock();
        if state.accepts_guest() && state.engine.guest_health() != health {
            state.engine.guest_health = health.into();
            let status = state.engine.clone();
            state.publish(Body::Engine(status));
        }
    }

    pub(super) fn stats(&self, sample: VmStatsSample) {
        let mut state = self.lock();
        if state.accepts_guest() {
            state.stats = Some(sample);
            let _ = state.stats_events.send(sample);
        }
    }

    pub(super) fn disk_sample(&self) -> Option<VmStatsSample> {
        let state = self.lock();
        matches!(
            state.engine.state(),
            marsa_proto::host::v1::EngineState::Ready | marsa_proto::host::v1::EngineState::Idle
        )
        .then_some(state.stats)
        .flatten()
    }

    pub(super) fn settings(&self) -> (Settings, SettingsSnapshot) {
        let state = self.lock();
        (state.settings.clone(), state.settings_snapshot.clone())
    }

    pub(super) fn settings_snapshot(&self) -> SettingsSnapshot {
        self.lock().settings_snapshot.clone()
    }

    pub(super) fn update_settings(
        &self,
        settings: Settings,
        text: String,
        revision: u64,
        idle_applied: bool,
    ) -> SettingsSnapshot {
        let mut state = self.lock();
        let restart_required = !idle_applied || restart_changed(&state.startup, &settings);
        state.settings = settings;
        let snapshot = SettingsSnapshot {
            settings_toml: text,
            revision,
            restart_required,
        };
        state.settings_snapshot = snapshot.clone();
        state.publish(Body::Settings(SettingsChanged { revision }));
        let _ = state.settings_events.send(snapshot.clone());
        snapshot
    }

    pub(super) fn runtime_applied(&self, applied: &Settings) {
        let mut state = self.lock();
        state.startup = applied.clone();
        let required = restart_changed(applied, &state.settings);
        if state.settings_snapshot.restart_required != required {
            state.settings_snapshot.restart_required = required;
            let snapshot = state.settings_snapshot.clone();
            state.publish(Body::Settings(SettingsChanged {
                revision: snapshot.revision,
            }));
            let _ = state.settings_events.send(snapshot);
        }
    }

    pub(super) fn diagnostic_snapshot(&self) -> diagnostics::Snapshot {
        let state = self.lock();
        diagnostics::Snapshot::new(
            &state.settings,
            &state.engine,
            &state.engine_history,
            state.network.as_ref(),
            state.settings_snapshot.restart_required,
        )
    }

    pub(super) fn watch_settings(&self, stop: watch::Receiver<bool>) -> Output<SettingsSnapshot> {
        let state = self.lock();
        stream_from(
            Some(state.settings_snapshot.clone()),
            state.settings_events.subscribe(),
            stop,
        )
    }

    pub(super) fn watch_stats(&self, stop: watch::Receiver<bool>) -> Output<VmStatsSample> {
        let state = self.lock();
        stream_from(state.stats, state.stats_events.subscribe(), stop)
    }

    fn subscribe(&self, stop: watch::Receiver<bool>) -> Output<DaemonEvent> {
        let state = self.lock();
        let first = state.envelope(Body::Snapshot(DaemonSnapshot {
            engine: Some(state.engine.clone()),
            settings_revision: state.settings_snapshot.revision,
            network: state.network.clone(),
            ..Default::default()
        }));
        stream_from(Some(first), state.events.subscribe(), stop)
    }

    fn lock(&self) -> MutexGuard<'_, StateAtCursor> {
        self.0.lock().unwrap_or_else(|lock| lock.into_inner())
    }
}

impl StateAtCursor {
    fn accepts_guest(&self) -> bool {
        use marsa_proto::host::v1::EngineState as Wire;
        matches!(
            self.engine.state(),
            Wire::Starting | Wire::Booting | Wire::Ready | Wire::Idle
        )
    }

    fn publish(&mut self, body: Body) {
        self.sequence += 1;
        let event = self.envelope(body);
        if let Some(Body::Engine(status)) = &event.body {
            if self.engine_history.len() == CAPACITY {
                self.engine_history.pop_front();
            }
            self.engine_history
                .push_back(diagnostics::EngineSample::new(
                    status,
                    event.sequence,
                    event.observed_at_unix_millis,
                ));
        }
        let _ = self.events.send(event);
    }

    fn envelope(&self, body: Body) -> DaemonEvent {
        DaemonEvent {
            stream_id: self.stream_id.clone(),
            sequence: self.sequence,
            observed_at_unix_millis: now(),
            body: Some(body),
        }
    }
}

fn stream_from<T: Clone + Send + 'static>(
    first: Option<T>,
    receiver: broadcast::Receiver<T>,
    stop: watch::Receiver<bool>,
) -> Output<T> {
    Box::pin(stream::unfold(
        (first, receiver, stop, false),
        |(first, mut receiver, mut stop, failed)| async move {
            if failed || *stop.borrow() {
                return None;
            }
            let item = match first {
                Some(first) => Ok(first),
                None => tokio::select! {
                    biased;
                    _ = stop.wait_for(|closing| *closing) => return None,
                    item = receiver.recv() => match item {
                        Ok(event) => Ok(event),
                        Err(broadcast::error::RecvError::Closed) => return None,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            Err(Status::resource_exhausted("stream fell behind; reconnect for a new snapshot"))
                        }
                    },
                },
            };
            let failed = item.is_err();
            Some((item, (None, receiver, stop, failed)))
        },
    ))
}

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}

pub(super) fn backend_name(backend: VmBackend) -> &'static str {
    match backend {
        VmBackend::Vz => "vz",
        VmBackend::Krun => "krun",
    }
}

#[tonic::async_trait]
impl Events for TypedApi {
    type SubscribeStream = Output<DaemonEvent>;

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        self.authorize(request)?;
        let _admission = self.inner.factory_reset.read()?;
        Ok(Response::new(
            self.inner.events.subscribe(self.inner.closing.subscribe()),
        ))
    }
}

fn restart_changed(applied: &Settings, desired: &Settings) -> bool {
    applied.vm.cpus != desired.vm.cpus
        || applied.vm.backend != desired.vm.backend
        || applied.vm.memory != desired.vm.memory
        || applied.disk.data_size_limit != desired.disk.data_size_limit
        || applied.disk.exclude_from_backup != desired.disk.exclude_from_backup
        || applied.shares != desired.shares
        || applied.network != desired.network
}

pub(super) fn mode(value: NetworkMode) -> WireMode {
    match value {
        NetworkMode::Auto => WireMode::Auto,
        NetworkMode::Nat => WireMode::Nat,
        NetworkMode::Userspace => WireMode::Userspace,
    }
}

pub(super) fn network_status(configured: NetworkMode, network: BootNetwork) -> NetworkStatus {
    let active = match network.active {
        NetworkSpec::Nat => WireMode::Nat,
        NetworkSpec::Userspace => WireMode::Userspace,
    };
    let reason = match network.result {
        NetworkResult::Unchecked | NetworkResult::NativeHealthy => None,
        NetworkResult::Unavailable => Some("probe_unavailable".into()),
        NetworkResult::Fallback(stage) => {
            Some(format!("native_{}_userspace_verified", stage_name(stage)))
        }
        NetworkResult::NativeDegraded(stage) => Some(format!("native_{}", stage_name(stage))),
        NetworkResult::AlternateFailed { native, alternate } => {
            let alternate = match alternate {
                AlternateFailure::Boot(stage) => engine::stage_name(stage),
                AlternateFailure::Egress(stage) => stage_name(stage),
                AlternateFailure::Transport => "transport_unavailable",
                AlternateFailure::Inconclusive => "probe_unavailable",
            };
            Some(format!(
                "native_{}_alternate_{alternate}",
                stage_name(native)
            ))
        }
        NetworkResult::TransportUnavailable => Some("transport_unavailable".into()),
    };
    NetworkStatus {
        configured_mode: mode(configured).into(),
        active_mode: active.into(),
        reason,
        ..Default::default()
    }
}

fn stage_name(stage: EgressStage) -> &'static str {
    match stage {
        EgressStage::Unspecified => "unknown",
        EgressStage::Healthy => "healthy",
        EgressStage::Dns => "dns",
        EgressStage::Route => "route",
        EgressStage::Tcp => "tcp",
        EgressStage::Tls => "tls",
        EgressStage::Http => "http",
        EgressStage::Policy => "policy",
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;

    #[test]
    fn successful_activation_clears_restart_only_for_the_configuration_it_loaded() {
        let original = Settings::default();
        let bus = Bus::new(original.clone()).unwrap();
        let mut desired = original.clone();
        desired.vm.memory = Some(1024 * 1024 * 1024);
        let revision = bus.settings_snapshot().revision + 1;
        assert!(
            bus.update_settings(desired.clone(), desired.to_toml().unwrap(), revision, true)
                .restart_required
        );

        bus.runtime_applied(&original);
        assert!(bus.settings_snapshot().restart_required);

        bus.runtime_applied(&desired);
        let applied = bus.settings_snapshot();
        assert_eq!(applied.revision, revision);
        assert!(!applied.restart_required);
    }

    #[tokio::test]
    async fn paused_subscriber_never_receives_a_gap_as_if_it_were_contiguous() {
        let bus = Bus::new(Settings::default()).unwrap();
        let (_closing, stop) = watch::channel(false);
        let mut stream = bus.subscribe(stop.clone());

        for _ in 0..=CAPACITY {
            bus.engine(&Transition {
                previous: State::Starting,
                current: State::Booting,
                failure: None,
                network: None,
            });
        }

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.sequence, 0);
        assert!(matches!(first.body, Some(Body::Snapshot(_))));
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );
        assert!(stream.next().await.is_none());

        let latest = bus.subscribe(stop).next().await.unwrap().unwrap();
        assert_eq!(latest.sequence, CAPACITY as u64 + 1);
        assert_eq!(latest.stream_id, first.stream_id);
    }
}
