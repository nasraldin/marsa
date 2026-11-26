//! Serve typed host APIs from one state and a composable VM supervisor.

mod agents;
mod bind_policy;
mod bind_prompts;
mod diagnostics;
mod disk;
mod disk_preview;
mod docker_discovery;
#[cfg(target_os = "macos")]
pub use docker_discovery::{ContextRemoval, ContextRemovalError, prepare_context_removal};
mod engine;
mod events;
mod factory_reset;
mod forwarding;
mod inventory_cache;
mod inventory_events;
/// Daemon state and actions exposed to its macOS menu bar item.
#[cfg(target_os = "macos")]
pub mod menu_bar;
mod meta;
mod native_dns;
mod network;
mod proxy_activation;
mod proxy_policy;
mod requests;
mod reset;
mod settings;
mod settings_feedback;
mod shares;
mod stats;
mod system_proxy;
mod volume_files;

use std::{future::Future, io, sync::Arc, time::Duration};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, Settings},
};
use marsa_proto::host::v1::volume_files_server::VolumeFilesServer;
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
    agent_policy::GrantStore,
    guest_control::{GuestControl, GuestEvent},
    host_events::{HostEvent, HostNotification},
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
    proxy_discovery: Arc<dyn system_proxy::Discovery>,
    guest_control: Option<Arc<GuestControl>>,
}

struct Shared {
    paths: MarsaPaths,
    uid: u32,
    events: events::Bus,
    ports: forwarding::Ports,
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
    registry_observations: tokio::sync::Semaphore,
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
    inventory: inventory_cache::InventoryCache,
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
        let inventory = inventory_cache::InventoryCache::open(paths.docker_inventory_cache())
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(Shared {
                agents: agents::State::new(paths.clone(), uid),
                grants: GrantStore::new(paths.clone(), uid),
                inventory,
                paths,
                uid,
                ports: forwarding::Ports::new(settings.network.allow_lan),
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
                registry_observations: tokio::sync::Semaphore::new(4),
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
            proxy_discovery: Arc::new(system_proxy::Platform),
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
        let selection = select_network(&settings, &mut spec, capabilities);
        events.backend(capabilities.name);
        self.capabilities = Some(capabilities);
        let supervisor = Supervisor::spawn_observed(
            backend,
            probe,
            spec,
            config,
            Arc::new(move |transition| events.engine(transition)),
            selection,
        );
        self.supervisor = Some(supervisor.clone());
        (self, supervisor)
    }

    /// Publish host notifications in arrival order and apply power changes to the active VM.
    ///
    /// The source closes when its watcher is dropped. Each sleep notification is
    /// acknowledged to the watcher once the supervisor finishes handling it.
    pub async fn watch_host_events(&self, mut source: mpsc::UnboundedReceiver<HostNotification>) {
        let (refresh, mut pending) = watch::channel(());
        let receive = async move {
            while let Some(mut notification) = source.recv().await {
                notification.begin();
                let event = notification.event;
                self.inner.events.host(event.into());
                if let Some(supervisor) = &self.supervisor {
                    let result = match event {
                        HostEvent::WillSleep => supervisor.host_will_sleep().await,
                        HostEvent::Woke => {
                            let result = supervisor.host_woke().await;
                            if result.is_ok() {
                                refresh.send_replace(());
                            }
                            result
                        }
                        HostEvent::NetworkChanged => {
                            refresh.send_replace(());
                            Ok(())
                        }
                    };
                    if let Err(error) = result {
                        tracing::warn!(?event, %error, "host power change failed");
                    }
                }
                notification.finish();
            }
        };
        // Resolver snapshots and guest replies can outlast an IOKit sleep
        // acknowledgement. Serialize DNS changes on their own coalescing queue.
        let apply = async {
            while pending.changed().await.is_ok() {
                let Some(supervisor) = &self.supervisor else {
                    continue;
                };
                self.observe_system_proxy().await;
                if let Err(error) = self.refresh_native_dns(supervisor).await {
                    tracing::warn!(%error, "native resolver refresh failed");
                    continue;
                }
                let (desired, snapshot) = self.inner.events.settings();
                if let Ok(Some(mut check)) = supervisor.live_network_check().await {
                    if desired.network.mode != NetworkMode::Auto {
                        check.observe_only();
                    }
                    let result = check.run().await;
                    // A new path invalidates both observations. The next iteration
                    // snapshots DNS and route policy again for that path.
                    let _edit = self.inner.edit.lock().await;
                    let (current, settings) = self.inner.events.settings();
                    if pending.has_changed().unwrap_or(false)
                        || settings.revision != snapshot.revision
                    {
                        continue;
                    }
                    if let Err(error) = supervisor
                        .apply_network_check(
                            check,
                            result,
                            current.network.mode == NetworkMode::Auto && !settings.restart_required,
                        )
                        .await
                    {
                        tracing::warn!(%error, "live network recovery could not complete");
                    }
                }
            }
        };
        tokio::join!(receive, apply);
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
        let owner = Arc::downgrade(&self.inner);
        let control = Arc::new(
            control
                .observe_active(move |event| match event {
                    GuestEvent::TrustRestartRequired => observations.trust_staged(),
                    GuestEvent::Health(health) => engine::publish_health(&observations, health),
                    GuestEvent::Stats(stats) => stats::publish(&observations, *stats),
                    GuestEvent::Port(port) => {
                        if let Some(owner) = owner.upgrade() {
                            owner.ports.event(port);
                        }
                    }
                    GuestEvent::PortsCleared => {
                        if let Some(owner) = owner.upgrade() {
                            owner.ports.clear();
                        }
                    }
                })
                .authorize_binds(bind_policy::Policy::new(
                    self.inner.events.clone(),
                    self.inner.share_policy.clone(),
                )),
        );
        let mut api = self
            .with_packaged_probe(capabilities, control.clone(), config, prepare)
            .await?;
        if let Some(supervisor) = api.supervisor.clone() {
            let connect: marsa_net::Connector = Arc::new(move || {
                let supervisor = supervisor.clone();
                Box::pin(async move {
                    let stream = supervisor.connect_running_published_port().await?;
                    Ok(Box::new(stream) as Box<dyn marsa_net::RelayStream>)
                })
            });
            api.inner.ports.serve(connect, api.inner.events.clone());
        }
        api.guest_control = Some(control);
        api.observe_system_proxy().await;
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
            let (effective, reserved, fixed_proxy) = proxy.launch_settings(desired)?;
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
            let selection = select_network(&effective, &mut spec, backend.capabilities());
            let mut network_policy = native_dns::configuration(
                &effective.network,
                spec.network,
                fixed_proxy.guest_configuration(),
            )?;
            if !effective.network.registry_mirrors.is_empty()
                || !effective.network.insecure_registries.is_empty()
            {
                network_policy
                    .get_or_insert_with(Default::default)
                    .registries = Some(marsa_proto::guest::v1::RegistryConfig {
                    mirrors: effective.network.registry_mirrors.clone(),
                    insecure: effective.network.insecure_registries.clone(),
                });
            }
            let bus = settings.clone();
            Ok(Launch {
                backend,
                spec,
                selection: selection.with_configured_mode(configured_mode),
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
            capabilities.pause_resume,
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
        self.spawn_inventory_refresh();
        self.spawn_inventory_observer();
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
            .add_service(
                StatsServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT)
                    .max_encoding_message_size(marsa_proto::host::DOCKER_INVENTORY_RESPONSE_LIMIT),
            )
            .add_service(NetworkServer::new(self.clone()).max_decoding_message_size(CONTROL_LIMIT))
            .add_service(
                VolumeFilesServer::new(self.clone())
                    .max_decoding_message_size(CONTROL_LIMIT)
                    .max_encoding_message_size(marsa_proto::guest::VOLUME_READ_CHUNK + 16),
            )
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

fn select_network(settings: &Settings, spec: &mut VmSpec, capabilities: Capabilities) -> Selection {
    spec.network = match settings.network.mode {
        NetworkMode::Auto | NetworkMode::Nat => NetworkSpec::Nat,
        NetworkMode::Userspace => NetworkSpec::Userspace,
    };
    let direct = settings.network.dns.is_empty()
        && settings.network.proxy.http.is_none()
        && settings.network.proxy.https.is_none();
    Selection::new(
        direct && (settings.network.mode != NetworkMode::Auto || capabilities.userspace_network),
    )
    .with_configured_mode(settings.network.mode)
}

#[cfg(test)]
#[path = "tests/proxy_policy.rs"]
mod proxy_policy_tests;
