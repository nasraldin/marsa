//! Serve typed host APIs from one state and a composable VM supervisor.

mod engine;
mod events;
mod meta;
mod requests;
mod settings;
mod stats;

use std::{future::Future, io, sync::Arc, time::Duration};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, Settings},
};
use marsa_proto::host::v1::{
    engine_server::EngineServer, events_server::EventsServer, meta_server::MetaServer,
    settings_server::SettingsServer, stats_server::StatsServer,
};
use marsa_vm::{Capabilities, Hypervisor, NetworkSpec, VmSpec};
use tokio::{
    net::UnixListener,
    sync::{mpsc, watch},
};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{
    Request, Status,
    transport::{Server, server::UdsConnectInfo},
};

use crate::{
    host_events::{HostEvent, HostNotification},
    supervisor::{GuestProbe, Supervisor, SupervisorConfig},
};

const CONTROL_LIMIT: usize = 4096;
const CONFIG_LIMIT: usize = marsa_core::settings::MAX_SETTINGS_BYTES + CONTROL_LIMIT;

/// State and services of one daemon, with optional native VM composition.
#[derive(Clone)]
pub struct TypedApi {
    inner: Arc<Shared>,
    supervisor: Option<Supervisor>,
    capabilities: Option<Capabilities>,
}

struct Shared {
    paths: MarsaPaths,
    uid: u32,
    events: events::Bus,
    engine_requests: requests::Cache<engine::Action, marsa_proto::host::v1::EngineStatus>,
    settings_requests: requests::Cache<(u64, Arc<str>), marsa_proto::host::v1::SettingsSnapshot>,
    edit: tokio::sync::Mutex<()>,
    closing: watch::Sender<bool>,
}

impl TypedApi {
    /// Initialize the authenticated services from the already-validated daemon settings.
    pub fn new(paths: MarsaPaths, uid: u32, settings: Settings) -> io::Result<Self> {
        let (closing, _) = watch::channel(false);
        Ok(Self {
            inner: Arc::new(Shared {
                paths,
                uid,
                events: events::Bus::new(settings)?,
                engine_requests: requests::Cache::new(),
                settings_requests: requests::Cache::new(),
                edit: tokio::sync::Mutex::new(()),
                closing,
            }),
            supervisor: None,
            capabilities: None,
        })
    }

    /// Install the native supervisor before any clients connect.
    ///
    /// The runtime provider builds a spec from persisted settings and supplies the
    /// real guest probe. The returned handle is also usable by other daemon subsystems.
    pub fn with_engine(
        mut self,
        backend: Arc<dyn Hypervisor>,
        probe: Arc<dyn GuestProbe>,
        mut spec: VmSpec,
        config: SupervisorConfig,
    ) -> (Self, Supervisor) {
        let events = self.inner.events.clone();
        let capabilities = backend.capabilities();
        let (settings, _) = events.settings();
        let automatic = settings.network.mode == NetworkMode::Auto
            && capabilities.userspace_network
            && settings.network.dns.is_empty()
            && settings.network.proxy.http.is_none()
            && settings.network.proxy.https.is_none();
        match settings.network.mode {
            NetworkMode::Auto | NetworkMode::Nat => spec.network = NetworkSpec::Nat,
            NetworkMode::Userspace => spec.network = NetworkSpec::Userspace,
        }

        events.backend(capabilities.name);
        self.capabilities = Some(capabilities);
        let supervisor = Supervisor::spawn_observed(
            backend,
            probe,
            spec,
            config,
            Arc::new(move |transition| events.engine(transition)),
            automatic,
        );
        self.supervisor = Some(supervisor.clone());
        (self, supervisor)
    }

    /// Publish host notifications in arrival order and apply power changes to the active VM.
    ///
    /// The source closes when its watcher is dropped. Each sleep notification is
    /// acknowledged to the watcher once the supervisor finishes handling it.
    pub async fn watch_host_events(&self, mut source: mpsc::UnboundedReceiver<HostNotification>) {
        while let Some(notification) = source.recv().await {
            let event = notification.event;
            self.inner.events.host(event.into());
            if let Some(supervisor) = &self.supervisor {
                let result = match event {
                    HostEvent::WillSleep => supervisor.host_will_sleep().await,
                    HostEvent::Woke => supervisor.host_woke().await,
                    HostEvent::NetworkChanged => Ok(()),
                };
                if let Err(error) = result {
                    tracing::warn!(?event, %error, "host power change failed");
                }
            }
            notification.finish();
        }
    }

    /// Serve the Unix RPCs until the caller requests shutdown.
    pub async fn serve(
        &self,
        listener: UnixListener,
        stop: impl Future<Output = ()>,
    ) -> Result<(), tonic::transport::Error> {
        Server::builder()
            .timeout(Duration::from_secs(180))
            .add_service(
                MetaServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT)
                    .max_encoding_message_size(CONTROL_LIMIT),
            )
            .add_service(EngineServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .add_service(
                SettingsServer::new(self.clone())
                    .max_decoding_message_size(CONFIG_LIMIT)
                    .max_encoding_message_size(CONFIG_LIMIT),
            )
            .add_service(EventsServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .add_service(StatsServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                stop.await;
                self.inner.closing.send_replace(true);
                if let Some(supervisor) = &self.supervisor
                    && let Err(error) = supervisor.stop().await
                {
                    tracing::warn!(%error, "engine shutdown failed");
                }
            })
            .await
    }

    fn authorize<T>(&self, request: Request<T>) -> Result<T, Status> {
        let uid = request
            .extensions()
            .get::<UdsConnectInfo>()
            .and_then(|info| info.peer_cred.map(|credentials| credentials.uid()));
        if uid != Some(self.inner.uid) {
            return Err(Status::permission_denied(
                "the caller does not own this daemon socket",
            ));
        }
        if *self.inner.closing.borrow() {
            return Err(Status::unavailable("daemon is shutting down"));
        }
        Ok(request.into_inner())
    }
}
