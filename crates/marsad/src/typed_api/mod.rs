//! Serve typed host APIs from one state and a composable VM supervisor.

mod agents;
mod bind_policy;
mod bind_prompts;
mod diagnostics;
mod disk;
mod disk_preview;
mod docker_discovery;
mod engine;
mod events;
mod factory_reset;
mod meta;
mod network;
mod proxy_activation;
mod proxy_policy;
mod requests;
mod reset;
mod settings;
mod shares;
mod stats;

use std::{future::Future, io, sync::Arc, time::Duration};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, Settings},
};
use marsa_proto::host::v1::{
    CollectDiagnosticsResponse, ComponentVersion, EnsureDockerContextResponse,
    agent_ops_server::AgentOpsServer, agents_server::AgentsServer,
    diagnostics_server::DiagnosticsServer, disk_server::DiskServer,
    docker_discovery_server::DockerDiscoveryServer, engine_server::EngineServer,
    events_server::EventsServer, meta_server::MetaServer, network_server::NetworkServer,
    settings_server::SettingsServer, shares_server::SharesServer, stats_server::StatsServer,
};
use marsa_share::{Allowlist, SharePolicy};
use marsa_vm::{Capabilities, Hypervisor, NetworkSpec, VmSpec};
use nix::unistd::{Uid, User};
use tokio::{net::UnixListener, sync::watch};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{
    Request, Status,
    transport::{Server, server::UdsConnectInfo},
};

use crate::{
    agent_policy::GrantStore,
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
    components: Arc<[ComponentVersion]>,
    docker_contexts: Option<Arc<docker_discovery::Registration>>,
    verify_keychain: Option<docker_discovery::VerifyKeychain>,
    proxy: Option<Arc<proxy_policy::RuntimeProxy>>,
    guest_control: Option<Arc<GuestControl>>,
}

struct Shared {
    paths: MarsaPaths,
    uid: u32,
    events: events::Bus,
    agents: agents::State,
    engine_requests: requests::Cache<engine::Action, marsa_proto::host::v1::EngineStatus>,
    proxy_requests: requests::Cache<
        marsa_proto::host::v1::ActivateProxyRequest,
        marsa_proto::host::v1::ProxyActivationResult,
    >,
    proxy_approval: std::sync::Mutex<Option<proxy_activation::Approval>>,
    settings_requests: requests::Cache<(u64, Arc<str>), marsa_proto::host::v1::SettingsSnapshot>,
    network_mode_requests: requests::Cache<NetworkMode, marsa_proto::host::v1::NetworkStatus>,
    diagnostics_requests: requests::Cache<bool, CollectDiagnosticsResponse>,
    settings_reset: reset::SettingsReset,
    factory_reset: factory_reset::FactoryReset,
    grants: GrantStore,
    reclaim_requests: requests::Cache<(), marsa_proto::host::v1::DiskReclaimResponse>,
    reclaim_active: tokio::sync::Mutex<()>,
    share_policy: SharePolicy,
    add_share_requests:
        requests::Cache<marsa_proto::host::v1::AddShareRequest, marsa_proto::host::v1::Share>,
    remove_share_requests: requests::Cache<String, marsa_proto::host::v1::RemoveShareResponse>,
    bind_resolutions: requests::Cache<(String, i32), marsa_proto::host::v1::ResolveBindResponse>,
    bind_edit: tokio::sync::Mutex<()>,
    docker_context_requests: requests::Cache<(), EnsureDockerContextResponse>,
    docker_keychain_requests: requests::Cache<(), bool>,
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
                grants: GrantStore::new(paths.clone(), uid),
                paths,
                uid,
                events: events::Bus::new(settings)?,
                engine_requests: requests::Cache::new(),
                proxy_requests: requests::Cache::new(),
                proxy_approval: std::sync::Mutex::new(None),
                settings_requests: requests::Cache::new(),
                network_mode_requests: requests::Cache::new(),
                diagnostics_requests: requests::Cache::new(),
                settings_reset: reset::SettingsReset::new(),
                factory_reset: factory_reset::FactoryReset::new(),
                reclaim_requests: requests::Cache::new(),
                reclaim_active: tokio::sync::Mutex::new(()),
                share_policy,
                add_share_requests: requests::Cache::new(),
                remove_share_requests: requests::Cache::new(),
                bind_resolutions: requests::Cache::new(),
                bind_edit: tokio::sync::Mutex::new(()),
                docker_context_requests: requests::Cache::new(),
                docker_keychain_requests: requests::Cache::new(),
                edit: tokio::sync::Mutex::new(()),
                closing,
            }),
            supervisor: None,
            capabilities: None,
            components: Arc::default(),
            docker_contexts: None,
            verify_keychain: None,
            proxy: None,
            guest_control: None,
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
            Selection::new(automatic).with_configured_mode(settings.network.mode),
        );
        self.supervisor = Some(supervisor.clone());
        (self, supervisor)
    }

    /// Install the runtime used by the bundled daemon.
    ///
    /// The factory runs on a blocking worker at every cold activation. It sees
    /// one settings snapshot for the whole attempt, including network fallback.
    pub async fn with_packaged_runtime(
        self,
        capabilities: Capabilities,
        control: GuestControl,
        config: SupervisorConfig,
        prepare: impl Fn(&Settings, &SharePolicy) -> io::Result<(Arc<dyn Hypervisor>, VmSpec)>
        + Send
        + Sync
        + 'static,
    ) -> io::Result<Self> {
        let observations = self.inner.events.clone();
        let control = Arc::new(
            control
                .observe_active(move |event| match event {
                    GuestEvent::Health(health) => engine::publish_health(&observations, health),
                    GuestEvent::Stats(stats) => stats::publish(&observations, *stats),
                    GuestEvent::Port(_) => (),
                })
                .authorize_binds(bind_policy::Policy::new(
                    self.inner.events.clone(),
                    self.inner.share_policy.clone(),
                )),
        );
        let mut api = self
            .with_packaged_probe(capabilities, control.clone(), config, prepare)
            .await?;
        api.guest_control = Some(control);
        Ok(api)
    }

    async fn with_packaged_probe(
        mut self,
        capabilities: Capabilities,
        control: Arc<dyn GuestProbe>,
        config: SupervisorConfig,
        prepare: impl Fn(&Settings, &SharePolicy) -> io::Result<(Arc<dyn Hypervisor>, VmSpec)>
        + Send
        + Sync
        + 'static,
    ) -> io::Result<Self> {
        let paths = self.inner.paths.clone();
        let uid = self.inner.uid;
        let proxy = tokio::task::spawn_blocking(move || proxy_policy::BootProxy::read(&paths, uid))
            .await
            .map_err(io::Error::other)??;
        let events = self.inner.events.clone();
        events.backend(capabilities.name);
        self.capabilities = Some(capabilities);
        let (desired, _) = events.settings();
        events.runtime_applied(&proxy.effective(desired));
        let proxy = Arc::new(proxy_policy::RuntimeProxy::new(proxy));
        self.proxy = Some(proxy.clone());
        let settings = events.clone();
        let share_policy = self.inner.share_policy.clone();
        let preparation = Arc::new(move || {
            let (desired, once) = settings.boot_configuration();
            let configured_mode = desired.network.mode;
            let (effective, reserved) = proxy.launch_settings(desired)?;
            let mut startup = effective.clone();
            startup.shares.extend(once.iter().cloned());
            let (backend, mut spec) = prepare(&startup, &share_policy)?;
            let mounted = Arc::new(
                Allowlist::from_paths(
                    share_policy.clone(),
                    spec.shares.iter().map(|share| &share.host_path),
                )
                .map_err(io::Error::other)?,
            );
            let automatic = select_network(&effective, &mut spec, backend.capabilities());
            let network_policy = proxy_policy::BootProxy::from_draft(&effective.network.proxy)?
                .guest_configuration();
            let bus = settings.clone();
            Ok(Launch {
                backend,
                spec,
                selection: Selection::new(automatic).with_configured_mode(configured_mode),
                network_policy,
                applied: Some(Arc::new(move || {
                    if !reserved {
                        bus.runtime_applied(&effective);
                    }
                    bus.activate_binds(mounted.clone(), &once);
                })),
            })
        });
        let supervisor = Supervisor::spawn_packaged(
            preparation,
            self.inner.paths.console_log(),
            control,
            config,
            Arc::new(move |transition| events.engine(transition)),
        );
        self.supervisor = Some(supervisor);
        Ok(self)
    }

    /// Attach the release versions reported by this daemon's installed guest.
    pub fn with_components(mut self, components: Vec<ComponentVersion>) -> Self {
        self.components = components.into();
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
            .add_service(DiskServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
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
            .add_service(
                DockerDiscoveryServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT)
                    .max_encoding_message_size(CONTROL_LIMIT),
            )
            .add_service(EventsServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .add_service(StatsServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .add_service(NetworkServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                stop.await;
                self.inner.closing.send_replace(true);
                let _factory = self.inner.factory_reset.close().await;
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
        self.inner
            .factory_reset
            .read()
            .map_err(|_| io::ErrorKind::WouldBlock)?;
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

#[cfg(test)]
#[path = "tests/proxy_policy.rs"]
mod proxy_policy_tests;
