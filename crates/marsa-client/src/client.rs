//! Open one authenticated transport and negotiate its host API compatibility.

use std::{
    error::Error as StdError,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use hyper_util::rt::TokioIo;
use marsa_proto::host::{AGENT_SOCKET_NAME, API_MAJOR, API_MINOR, socket_path};
use nix::unistd::geteuid;
use thiserror::Error;
use tokio::{
    net::UnixStream,
    time::{sleep, timeout},
};
use tonic::{
    Code, Request, Status,
    metadata::{Ascii, MetadataValue},
    transport::{Channel, Endpoint},
};
use tower::service_fn;

use crate::{
    api::{
        AgentOperation, AgentOperationRequest, AgentOperationResponse, OpenAgentSessionRequest,
        OpenAgentSessionResponse, VersionRequest, VersionResponse,
        agent_ops_client::AgentOpsClient, agents_client::AgentsClient,
        diagnostics_client::DiagnosticsClient, disk_client::DiskClient,
        docker_discovery_client::DockerDiscoveryClient, engine_client::EngineClient,
        events_client::EventsClient, meta_client::MetaClient, network_client::NetworkClient,
        settings_client::SettingsClient, shares_client::SharesClient, stats_client::StatsClient,
        updates_client::UpdatesClient,
    },
    events::EventSubscription,
    platform,
};

pub(super) const RETRY_DELAY: Duration = Duration::from_millis(100);
pub(super) const RPC_DEADLINE: Duration = Duration::from_secs(5);
pub(super) const STARTUP_DEADLINE: Duration = Duration::from_secs(30);

/// Errors returned by local daemon connection and event stream operations.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The process could not resolve the current user's daemon socket.
    #[error("could not resolve the default Marsa daemon socket")]
    Home,
    /// The typed API is only available to an unprivileged user.
    #[error("Marsa must run as an unprivileged user")]
    Root,
    /// The Unix connection could not safely be opened.
    #[error("could not connect to the local Marsa daemon socket: {0}")]
    Socket(#[source] io::Error),
    /// The client could not launch its matching daemon executable.
    #[error("could not start the Marsa daemon: {0}")]
    Launch(#[source] io::Error),
    /// The daemon did not accept an authenticated API connection in time.
    #[error("the Marsa daemon did not become ready before the connection deadline")]
    StartupTimeout,
    /// The daemon's major API version is incompatible with this client.
    #[error("incompatible Marsa daemon API major {daemon}; this client requires {supported}")]
    IncompatibleVersion {
        /// Daemon's advertised major API version.
        daemon: u32,
        /// Client's supported major API version.
        supported: u32,
    },
    /// The daemon lacks the required version service.
    #[error("Marsa daemon does not expose the required marsa.v1 version service")]
    MissingVersionService,
    /// The daemon transport closed or failed.
    #[error("Marsa daemon transport failed: {0}")]
    Transport(#[from] tonic::transport::Error),
    /// A daemon RPC returned a gRPC error.
    #[error("Marsa daemon API error: {0}")]
    Rpc(#[source] Box<Status>),
    /// The daemon failed to honor an event stream invariant.
    #[error("invalid Marsa daemon event stream: {0}")]
    EventProtocol(&'static str),
}

impl From<Status> for ClientError {
    fn from(status: Status) -> Self {
        Self::Rpc(Box::new(status))
    }
}

/// The location and startup policy for a per-user daemon connection.
#[derive(Debug, Clone)]
pub struct DaemonEndpoint {
    socket: PathBuf,
    autostart: bool,
}

impl DaemonEndpoint {
    /// Resolve the standard per-user socket and enable starting the bundled daemon on demand.
    pub fn discover() -> Result<Self, ClientError> {
        let home = std::env::home_dir().ok_or(ClientError::Home)?;
        let socket = socket_path(home).map_err(|_| ClientError::Home)?;
        Ok(Self {
            socket,
            autostart: true,
        })
    }

    /// Connect to an explicit absolute socket; the caller owns its process lifecycle.
    pub fn at(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            autostart: false,
        }
    }

    /// Open the daemon socket, wait for readiness, and check the API version.
    pub async fn connect(&self) -> Result<MarsaClient, ClientError> {
        self.connect_with(
            || async move {
                if self.autostart {
                    platform::launch_daemon().await
                } else {
                    Ok(())
                }
            },
            platform::open_socket,
        )
        .await
    }

    pub(super) async fn connect_with<L, O>(
        &self,
        launch: impl FnOnce() -> L,
        open_socket: impl Fn(PathBuf) -> O,
    ) -> Result<MarsaClient, ClientError>
    where
        L: Future<Output = io::Result<()>>,
        O: Future<Output = io::Result<UnixStream>>,
    {
        if geteuid().is_root() {
            return Err(ClientError::Root);
        }
        timeout(STARTUP_DEADLINE, async {
            let mut launch = Some(launch);
            loop {
                let connection = match open_socket(self.socket.clone()).await {
                    Ok(socket) => self.open(socket).await,
                    Err(error) => Err(ClientError::Socket(error)),
                };
                match connection {
                    Ok(client) => return Ok(client),
                    Err(ClientError::Socket(error))
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        if let Some(launch) = launch.take() {
                            launch().await.map_err(ClientError::Launch)?;
                        }
                    }
                    Err(ClientError::Socket(error)) if peer_disconnected(&error) => {}
                    Err(ClientError::Transport(error)) if peer_disconnected(&error) => {}
                    Err(ClientError::Rpc(status))
                        if status.code() == Code::Unavailable || peer_disconnected(&*status) => {}
                    Err(error) => return Err(error),
                }
                sleep(RETRY_DELAY).await;
            }
        })
        .await
        .map_err(|_| ClientError::StartupTimeout)?
    }

    async fn open(&self, socket: UnixStream) -> Result<MarsaClient, ClientError> {
        // Tonic must never reconnect a checked channel to a new daemon before
        // the client has authenticated it and negotiated its API version.
        let socket = Arc::new(Mutex::new(Some(socket)));
        let connector = service_fn(move |_| {
            let connection = socket
                .lock()
                .map_err(|_| io::Error::other("daemon socket connector was poisoned"))
                .and_then(|mut remaining| {
                    remaining.take().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotConnected,
                            "reconnect with a new Marsa channel",
                        )
                    })
                });
            std::future::ready(connection.map(TokioIo::new))
        });
        let channel = Endpoint::from_static("http://localhost")
            .connect_with_connector(connector)
            .await?;
        let mut meta = MetaClient::new(channel.clone());
        let version = timeout(RPC_DEADLINE, meta.version(VersionRequest {}))
            .await
            .map_err(|_| ClientError::StartupTimeout)?
            .map_err(|status| {
                if status.code() == Code::Unimplemented {
                    ClientError::MissingVersionService
                } else {
                    status.into()
                }
            })?
            .into_inner();
        if version.api_major != API_MAJOR {
            return Err(ClientError::IncompatibleVersion {
                daemon: version.api_major,
                supported: API_MAJOR,
            });
        }
        Ok(MarsaClient {
            daemon: self.clone(),
            channel,
            version,
        })
    }
}

pub(super) fn peer_disconnected(error: &(dyn StdError + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(error) = error.downcast_ref::<io::Error>()
            && matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::UnexpectedEof
            )
        {
            return true;
        }
        source = error.source();
    }
    false
}

/// A version-checked connection to one Marsa daemon generation.
///
/// Use the generated service clients for the daemon's typed RPCs. Connect
/// again through the endpoint after a restart. Event subscriptions
/// reconnect automatically and send a snapshot of the replacement daemon.
#[derive(Clone)]
pub struct MarsaClient {
    daemon: DaemonEndpoint,
    channel: Channel,
    version: VersionResponse,
}

macro_rules! service_clients {
    ($($method:ident: $client:ident => $doc:literal),* $(,)?) => {
        $(
            #[doc = $doc]
            pub fn $method(&self) -> $client<Channel> {
                $client::new(self.channel.clone())
            }
        )*
    }
}

impl MarsaClient {
    /// Information reported during the version check for this connection.
    pub fn version(&self) -> &VersionResponse {
        &self.version
    }

    /// The highest compatible API minor revision understood by both sides.
    pub fn api_minor(&self) -> u32 {
        API_MINOR.min(self.version.api_minor)
    }

    service_clients! {
        meta: MetaClient => "Access daemon version and capability queries.",
        engine: EngineClient => "Access engine lifecycle and status RPCs.",
        settings: SettingsClient => "Access settings reads, writes and watches.",
        shares: SharesClient => "Access shared folder RPCs.",
        docker_discovery: DockerDiscoveryClient => "Discover the Docker endpoint and register its context with host API minor 2 or later.",
        network: NetworkClient => "Access networking and forwarded port RPCs.",
        events: EventsClient => "Access the raw daemon event stream.",
        stats: StatsClient => "Access VM statistics.",
        disk: DiskClient => "Access disk accounting and maintenance.",
        updates: UpdatesClient => "Access application update RPCs.",
        diagnostics: DiagnosticsClient => "Access logs and diagnostic bundles.",
        agents: AgentsClient => "Access agent approvals and grants.",
    }

    /// Create a subscription that returns snapshots and ordered events across restarts.
    pub fn subscribe_events(&self) -> EventSubscription {
        EventSubscription::new(self.daemon.clone(), self.events())
    }
}

/// A capability-scoped connection that can submit agent operations.
#[derive(Clone)]
pub struct AgentClient {
    operations: AgentOpsClient<Channel>,
    authorization: MetadataValue<Ascii>,
    client_id: String,
    project: String,
}

impl MarsaClient {
    /// Register an agent from the trusted owner connection and open its restricted endpoint.
    pub async fn delegate_agent(
        &self,
        request: OpenAgentSessionRequest,
    ) -> Result<AgentClient, ClientError> {
        let client_id = request.client_id.clone();
        let session = self.agents().open_session(request).await?.into_inner();
        AgentClient::connect_at(
            self.daemon.socket.with_file_name(AGENT_SOCKET_NAME),
            client_id,
            session,
        )
        .await
    }
}

impl AgentClient {
    /// Open the owner-checked local endpoint using a previously issued delegation.
    pub async fn connect_at(
        socket: PathBuf,
        client_id: String,
        session: OpenAgentSessionResponse,
    ) -> Result<Self, ClientError> {
        if geteuid().is_root() {
            return Err(ClientError::Root);
        }
        if session.token.len() != 64 || !session.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Status::unauthenticated("invalid agent session capability").into());
        }
        let mut authorization: MetadataValue<Ascii> =
            format!("Bearer {}", session.token)
                .parse()
                .map_err(|_| Status::unauthenticated("invalid agent session capability"))?;
        authorization.set_sensitive(true);
        let channel = Endpoint::from_static("http://localhost")
            .connect_timeout(RPC_DEADLINE)
            .connect_with_connector(service_fn(move |_| {
                let socket = socket.clone();
                async move { platform::open_socket(socket).await.map(TokioIo::new) }
            }))
            .await?;
        Ok(Self {
            operations: AgentOpsClient::new(channel),
            authorization,
            client_id,
            project: session.project_path,
        })
    }

    /// Execute a typed operation with the registered client and project scope.
    pub async fn execute(
        &mut self,
        request_id: String,
        operation: AgentOperation,
        arguments_json: String,
    ) -> Result<AgentOperationResponse, ClientError> {
        let mut request = Request::new(AgentOperationRequest {
            request_id,
            client_id: self.client_id.clone(),
            project_path: self.project.clone(),
            operation: operation.into(),
            arguments_json,
        });
        request
            .metadata_mut()
            .insert("authorization", self.authorization.clone());
        Ok(self.operations.execute(request).await?.into_inner())
    }
}
