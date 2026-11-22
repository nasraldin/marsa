//! Serve typed host APIs from one state and a composable VM supervisor.

mod agents;
mod diagnostics;
mod engine;
mod events;
mod meta;
mod requests;
mod settings;
mod shares;
mod stats;

use std::{future::Future, io, sync::Arc, time::Duration};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, Settings},
};
use marsa_proto::host::v1::{
    CollectDiagnosticsResponse, agent_ops_server::AgentOpsServer, agents_server::AgentsServer,
    diagnostics_server::DiagnosticsServer, engine_server::EngineServer,
    events_server::EventsServer, meta_server::MetaServer, settings_server::SettingsServer,
    shares_server::SharesServer, stats_server::StatsServer,
};
use marsa_share::SharePolicy;
use marsa_vm::{Capabilities, Hypervisor, NetworkSpec, VmSpec};
use nix::unistd::{Uid, User};
use tokio::{net::UnixListener, sync::watch};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{
    Request, Status,
    transport::{Server, server::UdsConnectInfo},
};

use crate::{
    guest_control::{GuestControl, GuestEvent},
    supervisor::{
        DockerStream, GuestProbe, Supervisor, SupervisorConfig, launch::Launch, network::Selection,
    },
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
    agents: agents::State,
    engine_requests: requests::Cache<engine::Action, marsa_proto::host::v1::EngineStatus>,
    settings_requests: requests::Cache<(u64, Arc<str>), marsa_proto::host::v1::SettingsSnapshot>,
    diagnostics_requests: requests::Cache<bool, CollectDiagnosticsResponse>,
    share_policy: SharePolicy,
    add_share_requests:
        requests::Cache<marsa_proto::host::v1::AddShareRequest, marsa_proto::host::v1::Share>,
    remove_share_requests: requests::Cache<String, marsa_proto::host::v1::RemoveShareResponse>,
    edit: tokio::sync::Mutex<()>,
    closing: watch::Sender<bool>,
}

impl TypedApi {
    /// Initialize the authenticated services from the already-validated daemon settings.
    pub fn new(paths: MarsaPaths, uid: u32, settings: Settings) -> io::Result<Self> {
        let home =
            std::env::home_dir().ok_or_else(|| io::Error::other("could not find the user home"))?;
        let account = User::from_uid(Uid::from_raw(uid))?
            .ok_or_else(|| io::Error::other("could not find the daemon account"))?;
        let share_policy =
            SharePolicy::for_account(&home, &account.dir, &paths).map_err(io::Error::other)?;
        let (closing, _) = watch::channel(false);
        Ok(Self {
            inner: Arc::new(Shared {
                agents: agents::State::new(paths.clone(), uid),
                paths,
                uid,
                events: events::Bus::new(settings)?,
                engine_requests: requests::Cache::new(),
                settings_requests: requests::Cache::new(),
                diagnostics_requests: requests::Cache::new(),
                share_policy,
                add_share_requests: requests::Cache::new(),
                remove_share_requests: requests::Cache::new(),
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
        let automatic = select_network(&settings, &mut spec, capabilities);
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

    /// Install the runtime used by the bundled daemon.
    ///
    /// The factory runs on a blocking worker at every cold activation. It sees
    /// one settings snapshot for the whole attempt, including network fallback.
    pub fn with_packaged_runtime(
        mut self,
        capabilities: Capabilities,
        control: GuestControl,
        config: SupervisorConfig,
        prepare: impl Fn(&Settings) -> io::Result<(Arc<dyn Hypervisor>, VmSpec)> + Send + Sync + 'static,
    ) -> Self {
        let events = self.inner.events.clone();
        events.backend(capabilities.name);
        self.capabilities = Some(capabilities);
        let settings = events.clone();
        let preparation = Arc::new(move || {
            let (desired, _) = settings.settings();
            let (backend, mut spec) = prepare(&desired)?;
            let automatic = select_network(&desired, &mut spec, backend.capabilities());
            let bus = settings.clone();
            Ok(Launch {
                backend,
                spec,
                selection: Selection::new(automatic),
                applied: Some(Arc::new(move || bus.runtime_applied(&desired))),
            })
        });
        let observations = self.inner.events.clone();
        let control = Arc::new(control.observe_active(move |event| match event {
            GuestEvent::Health(health) => engine::publish_health(&observations, health),
            GuestEvent::Stats(stats) => stats::publish(&observations, *stats),
            GuestEvent::Port(_) => (),
        }));
        let supervisor = Supervisor::spawn_packaged(
            preparation,
            self.inner.paths.console_log(),
            control,
            config,
            Arc::new(move |transition| events.engine(transition)),
        );
        self.supervisor = Some(supervisor);
        self
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
            .add_service(
                DiagnosticsServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT)
                    .max_encoding_message_size(CONTROL_LIMIT),
            )
            .add_service(
                SharesServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT)
                    .max_encoding_message_size(CONFIG_LIMIT),
            )
            .add_service(
                AgentsServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT * 2)
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

    /// Serve delegated agent operations on a separate owner-protected Unix socket.
    pub async fn serve_agents(
        &self,
        listener: UnixListener,
    ) -> Result<(), tonic::transport::Error> {
        let mut closing = self.inner.closing.subscribe();
        Server::builder()
            .add_service(
                AgentOpsServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT * 3)
                    .max_encoding_message_size(CONTROL_LIMIT * 10),
            )
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = closing.wait_for(|closing| *closing).await;
            })
            .await
    }

    /// Activate the guest on demand and open a Docker engine stream.
    pub async fn connect_docker(&self) -> io::Result<DockerStream> {
        let supervisor = self
            .supervisor
            .as_ref()
            .ok_or(io::ErrorKind::NotConnected)?;
        supervisor.connect_docker().await
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

fn select_network(settings: &Settings, spec: &mut VmSpec, capabilities: Capabilities) -> bool {
    spec.network = match settings.network.mode {
        NetworkMode::Auto | NetworkMode::Nat => NetworkSpec::Nat,
        NetworkMode::Userspace => NetworkSpec::Userspace,
    };
    settings.network.mode == NetworkMode::Auto
        && capabilities.userspace_network
        && settings.network.dns.is_empty()
        && settings.network.proxy.http.is_none()
        && settings.network.proxy.https.is_none()
}
