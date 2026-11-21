//! Capture state and subscribe at one cursor under the same lock as publishers.

use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    time::SystemTime,
};

use futures_util::{Stream, stream};
use marsa_core::settings::{Settings, VmBackend};
use marsa_proto::host::v1::{
    DaemonEvent, DaemonSnapshot, EngineHealthState, EngineStatus, SettingsChanged,
    SettingsSnapshot, SubscribeRequest, VmStatsSample, daemon_event::Body, events_server::Events,
};
use tokio::sync::{broadcast, watch};
use tonic::{Request, Response, Status};

use crate::supervisor::{State, Transition};

use super::{TypedApi, engine};

const CAPACITY: usize = 128;
pub(super) type Output<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[derive(Clone)]
pub(super) struct Bus(Arc<Mutex<StateAtCursor>>);

struct StateAtCursor {
    stream_id: String,
    sequence: u64,
    engine: EngineStatus,
    startup: Settings,
    settings: Settings,
    settings_snapshot: SettingsSnapshot,
    stats: Option<VmStatsSample>,
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
            startup: settings.clone(),
            settings,
            settings_snapshot: SettingsSnapshot {
                settings_toml: initial_toml,
                revision: initial_revision,
                restart_required: false,
            },
            stats: None,
            events: broadcast::channel(CAPACITY).0,
            settings_events: broadcast::channel(8).0,
            stats_events: broadcast::channel(CAPACITY).0,
        }))))
    }

    pub(super) fn backend(&self, name: &str) {
        self.lock().engine.backend = name.into();
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
        let restart_required = !idle_applied
            || state.startup.vm.cpus != settings.vm.cpus
            || state.startup.vm.backend != settings.vm.backend
            || state.startup.vm.memory != settings.vm.memory
            || state.startup.disk.data_size_limit != settings.disk.data_size_limit
            || state.startup.shares != settings.shares
            || state.startup.network != settings.network;
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
        let _ = self.events.send(self.envelope(body));
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
        Ok(Response::new(
            self.inner.events.subscribe(self.inner.closing.subscribe()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;

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
