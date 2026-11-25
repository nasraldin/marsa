//! Keep daemon, Docker and selected resource streams scoped to the desktop window.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    future::Future,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(target_os = "macos")]
use marsa_client::docker::ContainerSummary;
use marsa_client::{
    ClientError, DaemonEndpoint, EventSubscription,
    api::{DaemonEvent, EngineActionRequest, EngineState, daemon_event::Body},
    compose::{ComposeCommand, ComposeError, ComposeOutput, ComposeStream},
    docker::{
        ContainerAction, ContainerDetails, ContainerId, ContainerRemoval, DockerClient,
        DockerError, DockerLogs, DockerStats, EnvironmentReveal, ExecControl, ExternalEndpoint,
        ImageDetails, ImageId, ImagePull, ImageReference, LogLine, MAX_TERMINAL_INPUT,
        PullProgress, RevealedEnvironment, StatsSample, TerminalSize, VolumeUsage,
    },
    runtime::{Background, Cancellation, Receiver, Sender, Task, delay, queue},
};

use crate::store::{ResourceSnapshot, Store};

#[cfg(target_os = "macos")]
mod metrics;

pub use marsa_client::docker::ExecOutput as TerminalOutput;

const RETRY: Duration = Duration::from_secs(1);
const LOG_RATE: Duration = Duration::from_millis(16);
const STATS_RATE: Duration = Duration::from_millis(250);
const LOG_LINES: usize = 512;
const LOG_BYTES: usize = 128 * 1024;

/// Commands the desktop can dispatch without an optimistic store write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start, stop, or restart the engine.
    Engine {
        /// Requested lifecycle transition.
        action: EngineAction,
        /// Caller-provided UUID for daemon deduplication.
        request_id: String,
    },
    /// Operate an existing container by full ID.
    Container {
        /// Container selected by the user.
        id: ContainerId,
        /// Requested lifecycle transition.
        action: ContainerAction,
    },
    /// Remove a container after the view confirms the selected removal mode.
    RemoveContainer(ContainerRemoval),
}

/// Daemon lifecycle transitions available to desktop views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineAction {
    /// Start the engine.
    Start,
    /// Stop the engine.
    Stop,
    /// Restart the engine.
    Restart,
}

/// Errors returned to callers of desktop services.
#[derive(Debug)]
pub enum ServiceError {
    /// Daemon startup, subscription, or RPC failed.
    Daemon(ClientError),
    /// Docker socket operation or response failed.
    Docker(DockerError),
    /// A selected, bundled Compose operation failed.
    Compose(ComposeError),
    /// The background runtime could not start.
    Runtime(io::Error),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daemon(error) => fmt::Display::fmt(error, f),
            Self::Docker(error) => fmt::Display::fmt(error, f),
            Self::Compose(error) => fmt::Display::fmt(error, f),
            Self::Runtime(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            Self::Daemon(error) => error,
            Self::Docker(error) => error,
            Self::Compose(error) => error,
            Self::Runtime(error) => error,
        })
    }
}

impl From<ClientError> for ServiceError {
    fn from(error: ClientError) -> Self {
        Self::Daemon(error)
    }
}

impl From<DockerError> for ServiceError {
    fn from(error: DockerError) -> Self {
        Self::Docker(error)
    }
}

/// A desktop window's application store and all of its asynchronous I/O.
pub struct Services {
    session: Session<LocalClients>,
}

impl Services {
    /// Start subscribing through the standard per-user daemon and Docker endpoints.
    pub fn start() -> Result<Self, ServiceError> {
        Self::start_with_inventory(true)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn start_notifications() -> Result<Self, ServiceError> {
        Self::start_with_inventory(false)
    }

    fn start_with_inventory(inventory: bool) -> Result<Self, ServiceError> {
        let clients = LocalClients {
            daemon: DaemonEndpoint::discover()?,
            docker: DockerClient::discover()?,
        };
        Ok(Self {
            session: Session::start(clients, inventory)?,
        })
    }

    /// Cancel all window work even when a view still retains a service handle.
    pub fn close(&self) {
        self.session.close();
    }

    /// Read normalized state through the window's store.
    pub fn store(&self) -> &Store {
        &self.session.store
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn request<T: Send + 'static, F>(
        &self,
        operation: impl FnOnce(marsa_client::MarsaClient) -> F + Send + 'static,
    ) -> Pending<T>
    where
        F: Future<Output = Result<T, ClientError>> + Send + 'static,
    {
        let endpoint = self.session.backend.daemon.clone();
        let (reply, completion) = queue(1);
        let task = self.session.runtime.spawn(async move {
            let result = match endpoint.connect().await {
                Ok(client) => operation(client).await,
                Err(error) => Err(error),
            };
            reply.send(result.map_err(ServiceError::from)).await;
        });
        Pending {
            completion,
            _task: task,
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn configure_login(
        &self,
        action: marsa_client::login::LoginAction,
    ) -> Pending<marsa_client::login::LoginStatus> {
        self.pending(async move {
            marsa_client::login::configure_login(action)
                .await
                .map_err(ServiceError::Runtime)
        })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn external_inventory(
        &self,
    ) -> Pending<Vec<marsa_client::docker::ExternalInventory>> {
        self.pending(async { Ok(marsa_client::docker::discover_external_inventories().await) })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn inspect_external(
        &self,
        endpoint: marsa_client::docker::ExternalEndpoint,
        id: ContainerId,
    ) -> Pending<ContainerDetails> {
        self.pending(async move { Ok(endpoint.inspect(&id).await?) })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn act_external(
        &self,
        endpoint: marsa_client::docker::ExternalEndpoint,
        id: ContainerId,
        action: ContainerAction,
    ) -> Pending<(bool, Option<Vec<ContainerSummary>>)> {
        self.pending(async move {
            let failed = endpoint.act(&id, action).await.is_err();
            let containers = endpoint.containers().await.ok();
            Ok((failed, containers))
        })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn remove_external(
        &self,
        endpoint: ExternalEndpoint,
        removal: ContainerRemoval,
    ) -> Pending<(bool, Option<Vec<ContainerSummary>>)> {
        self.pending(async move {
            let failed = endpoint
                .management_client()
                .remove_container(removal)
                .await
                .is_err();
            Ok((failed, endpoint.containers().await.ok()))
        })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn external_logs(
        &self,
        endpoint: marsa_client::docker::ExternalEndpoint,
        id: ContainerId,
    ) -> Batches<LogLine> {
        log_batches(&self.session.runtime, async move {
            Ok(endpoint.logs(&id).await?)
        })
    }

    /// Dispatch a command on the worker and receive its completion separately from state updates.
    pub fn dispatch(&self, command: Command) -> PendingCommand {
        self.session.dispatch(command)
    }

    /// Refresh inventory from the already-running engine on demand.
    pub fn refresh(&self) -> PendingCommand {
        let inventory = self.session.inventory.clone();
        let (reply, completion) = queue(1);
        let task = self.session.runtime.spawn(async move {
            inventory.send(Refresh::Requested(reply)).await;
        });
        Pending {
            completion,
            _task: task,
        }
    }

    /// Read bounded details for the selected container on the I/O worker.
    pub fn inspect(&self, id: ContainerId) -> Pending<ContainerDetails> {
        let backend = Arc::clone(&self.session.backend);
        let (reply, completion) = queue(1);
        let task = self.session.runtime.spawn(async move {
            reply.send(backend.inspect(&id).await).await;
        });
        Pending {
            completion,
            _task: task,
        }
    }

    /// Retrieve one environment value explicitly revealed by the owner.
    pub fn reveal_environment(
        &self,
        reveal: EnvironmentReveal,
    ) -> Pending<Option<RevealedEnvironment>> {
        let docker = self.session.backend.docker.clone();
        self.pending(async move { Ok(docker.reveal_environment(reveal).await?) })
    }

    /// Read metadata for an existing image.
    pub fn inspect_image(
        &self,
        id: ImageId,
        endpoint: Option<ExternalEndpoint>,
    ) -> Pending<ImageDetails> {
        let docker = endpoint
            .map(|endpoint| endpoint.management_client())
            .unwrap_or_else(|| self.session.backend.docker.clone());
        self.pending(async move { Ok(docker.inspect_image(&id).await?) })
    }

    /// Read the layer history for an existing image.
    pub fn image_history(
        &self,
        id: ImageId,
        endpoint: Option<ExternalEndpoint>,
    ) -> Pending<Vec<marsa_client::docker::ImageHistoryLayer>> {
        let docker = endpoint
            .map(|endpoint| endpoint.management_client())
            .unwrap_or_else(|| self.session.backend.docker.clone());
        self.pending(async move { Ok(docker.image_history(&id).await?) })
    }

    /// Run the explicitly selected Compose project and stream bounded output.
    pub fn compose(
        &self,
        command: ComposeCommand,
        endpoint: Option<ExternalEndpoint>,
    ) -> Batches<ComposeOutput> {
        batch_stream(
            &self.session.runtime,
            async move {
                command
                    .start_on(endpoint)
                    .await
                    .map_err(ServiceError::Compose)
            },
            Rate {
                interval: Duration::from_millis(100),
                maximum: 64,
                budget: LOG_BYTES,
                latest: false,
            },
            |output| output.text.len(),
        )
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn managed_activity(&self) -> Pending<Vec<marsa_client::docker::DockerActivity>> {
        let daemon = self.session.backend.daemon.clone();
        self.pending(async move { Ok(daemon.connect().await?.recent_docker_activity().await?) })
    }

    pub(crate) fn resource_docker(&self, endpoint: Option<ExternalEndpoint>) -> DockerClient {
        endpoint.map_or_else(
            || self.session.backend.docker.clone(),
            |value| value.management_client(),
        )
    }

    /// Pull a public image and stream layer progress. Dropping the batches cancels it.
    pub fn pull_image(
        &self,
        image: ImageReference,
        endpoint: Option<ExternalEndpoint>,
    ) -> Batches<PullProgress> {
        let docker = endpoint
            .map(|endpoint| endpoint.management_client())
            .unwrap_or_else(|| self.session.backend.docker.clone());
        batch_stream(
            &self.session.runtime,
            async move { Ok(docker.pull_image(&image).await?) },
            Rate {
                interval: Duration::from_millis(100),
                maximum: 64,
                budget: LOG_BYTES,
                latest: false,
            },
            |progress| {
                progress.id.as_ref().map_or(0, String::len)
                    + progress.status.as_ref().map_or(0, String::len)
            },
        )
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn external_containers(
        &self,
        endpoint: ExternalEndpoint,
    ) -> Pending<Vec<ContainerSummary>> {
        self.pending(async move { Ok(endpoint.containers().await?) })
    }

    /// Measure volume disk usage when requested by the storage view.
    pub fn volume_usage(
        &self,
        endpoint: Option<ExternalEndpoint>,
    ) -> Pending<BTreeMap<String, VolumeUsage>> {
        let docker = self.resource_docker(endpoint);
        self.pending(async move { Ok(docker.volume_usage().await?) })
    }

    pub(crate) fn pending<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T, ServiceError>> + Send + 'static,
    ) -> Pending<T> {
        let (reply, completion) = queue(1);
        let task = self.session.runtime.spawn(async move {
            reply.send(work.await).await;
        });
        Pending {
            completion,
            _task: task,
        }
    }

    /// Open a bounded interactive terminal for the selected container.
    pub fn terminal(
        &self,
        id: ContainerId,
        endpoint: Option<ExternalEndpoint>,
        columns: u16,
        rows: u16,
    ) -> Terminal {
        let docker = self.session.backend.docker.clone();
        let (write, controls) = queue(32);
        let (publish, output) = queue(16);
        let task = self.session.runtime.spawn(async move {
            let result = async {
                let size = TerminalSize::new(columns, rows)?;
                let session = match endpoint {
                    Some(endpoint) => endpoint.start_terminal(&id, size).await?,
                    None => docker.start_terminal(&id, size).await?,
                };
                session.drive(controls, &publish).await
            }
            .await;
            if let Err(error) = result {
                publish.send(Err(error)).await;
            }
        });
        let input = TerminalInput {
            write,
            cancellation: task.cancellation(),
        };
        Terminal {
            input,
            output,
            _task: task,
        }
    }

    /// Follow a container's log output in ordered, bounded batches.
    pub fn logs(&self, id: ContainerId) -> Batches<LogLine> {
        self.session.logs(id)
    }

    /// Follow the latest container stats sample at a capped display rate.
    pub fn stats(
        &self,
        id: ContainerId,
        endpoint: Option<ExternalEndpoint>,
    ) -> Batches<StatsSample> {
        self.session.stats(id, endpoint)
    }
}

/// Owned interactive attachment for a single terminal tab.
pub struct Terminal {
    input: TerminalInput,
    output: Receiver<Result<TerminalOutput, DockerError>>,
    _task: Task,
}

impl Terminal {
    /// Clone the input side for native key and resize callbacks.
    pub fn input(&self) -> TerminalInput {
        self.input.clone()
    }

    /// Read output in wire order. Closing the attachment ends the stream.
    pub async fn next(&mut self) -> Option<Result<TerminalOutput, ServiceError>> {
        self.output
            .next()
            .await
            .map(|result| result.map_err(ServiceError::Docker))
    }
}

/// Nonblocking input and explicit cancellation for a terminal tab.
#[derive(Clone)]
pub struct TerminalInput {
    write: Sender<ExecControl>,
    cancellation: Cancellation,
}

impl TerminalInput {
    /// Send a bounded raw input fragment. False means invalid, full, or closed.
    pub fn write(&self, bytes: Vec<u8>) -> bool {
        bytes.len() <= MAX_TERMINAL_INPUT && self.write.try_send(ExecControl::Input(bytes))
    }

    /// Queue a bounded native resize request without blocking the UI.
    pub fn resize(&self, columns: u16, rows: u16) -> bool {
        TerminalSize::new(columns, rows)
            .is_ok_and(|size| self.write.try_send(ExecControl::Resize(size)))
    }

    /// Stop forwarding and close the socket, including during the upgrade.
    pub fn close(&self) {
        self.cancellation.cancel();
    }
}

/// An in-flight command with no response payload.
pub type PendingCommand = Pending<()>;

/// An in-flight client operation. Keep this handle while waiting for the result.
pub struct Pending<T> {
    completion: Receiver<Result<T, ServiceError>>,
    _task: Task,
}

impl<T> Pending<T> {
    /// Read its result or none if closing the window cancelled the operation.
    pub async fn result(&mut self) -> Option<Result<T, ServiceError>> {
        self.completion.next().await
    }
}

/// Bounded updates for one log or stats view. Dropping it releases the subscription.
pub struct Batches<T> {
    updates: Receiver<Result<Vec<T>, ServiceError>>,
    _producer: Task,
    _delivery: Task,
}

impl<T> Batches<T> {
    /// Try to take a ready batch without waiting or doing I/O on the caller's thread.
    pub fn try_next(&mut self) -> Option<Result<Vec<T>, ServiceError>> {
        self.updates.try_next()
    }

    /// Wait asynchronously for the next batch or the end of the stream.
    pub async fn next(&mut self) -> Option<Result<Vec<T>, ServiceError>> {
        self.updates.next().await
    }
}

struct Session<B> {
    backend: Arc<B>,
    store: Store,
    _daemon: Task,
    inventory: Sender<Refresh>,
    _inventory: Task,
    runtime: Background,
}

impl<B: Clients> Session<B> {
    fn start(backend: B, watch_inventory: bool) -> Result<Self, ServiceError> {
        let runtime = Background::new().map_err(ServiceError::Runtime)?;
        let backend = Arc::new(backend);
        let store = Store::default();
        let (inventory, requests) = queue(8);
        let refresh = runtime.spawn(refresh_inventory(
            Arc::clone(&backend),
            store.clone(),
            requests,
        ));
        let daemon = runtime.spawn(watch_daemon(
            Arc::clone(&backend),
            store.clone(),
            watch_inventory.then(|| inventory.clone()),
        ));
        Ok(Self {
            backend,
            store,
            _daemon: daemon,
            inventory,
            _inventory: refresh,
            runtime,
        })
    }

    fn close(&self) {
        self.runtime.close();
    }

    fn dispatch(&self, command: Command) -> PendingCommand {
        let backend = Arc::clone(&self.backend);
        let (reply, completion) = queue(1);
        let task = self.runtime.spawn(async move {
            reply.send(backend.dispatch(command).await).await;
        });
        Pending {
            completion,
            _task: task,
        }
    }

    fn logs(&self, id: ContainerId) -> Batches<LogLine> {
        let backend = Arc::clone(&self.backend);
        log_batches(&self.runtime, async move { backend.logs(&id).await })
    }

    fn stats(&self, id: ContainerId, endpoint: Option<ExternalEndpoint>) -> Batches<StatsSample> {
        let backend = Arc::clone(&self.backend);
        match endpoint {
            Some(endpoint) => {
                stats_batches(&self.runtime, async move { Ok(endpoint.stats(&id).await?) })
            }
            None => stats_batches(&self.runtime, async move { backend.stats(&id).await }),
        }
    }
}

fn stats_batches<F: Feed<StatsSample>>(
    runtime: &Background,
    open: impl Future<Output = Result<F, ServiceError>> + Send + 'static,
) -> Batches<StatsSample> {
    batch_stream(
        runtime,
        open,
        Rate {
            interval: STATS_RATE,
            maximum: 64,
            budget: usize::MAX,
            latest: true,
        },
        |_| 1,
    )
}

fn log_batches<F: Feed<LogLine>>(
    runtime: &Background,
    open: impl Future<Output = Result<F, ServiceError>> + Send + 'static,
) -> Batches<LogLine> {
    batch_stream(
        runtime,
        open,
        Rate {
            interval: LOG_RATE,
            maximum: LOG_LINES,
            budget: LOG_BYTES,
            latest: false,
        },
        |line| line.text.len(),
    )
}

struct Rate {
    interval: Duration,
    maximum: usize,
    budget: usize,
    latest: bool,
}

fn batch_stream<T, F>(
    runtime: &Background,
    open: impl Future<Output = Result<F, ServiceError>> + Send + 'static,
    rate: Rate,
    weight: fn(&T) -> usize,
) -> Batches<T>
where
    T: Send + 'static,
    F: Feed<T>,
{
    let (output, mut raw) = queue(64);
    let (publish, updates) = queue(2);
    let producer = runtime.spawn(async move {
        let mut feed = match open.await {
            Ok(feed) => feed,
            Err(error) => {
                output.send(Err(error)).await;
                return;
            }
        };
        loop {
            match feed.pull().await {
                Ok(Some(item)) => {
                    if !output.send(Ok(item)).await {
                        return;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    output.send(Err(error)).await;
                    return;
                }
            }
        }
    });
    let delivery = runtime.spawn(async move {
        while let Some(batch) = raw
            .batch(rate.maximum, rate.interval, rate.budget, |item| {
                item.as_ref().map_or(0, &weight)
            })
            .await
        {
            let mut ready = Vec::new();
            let mut failure = None;
            for item in batch {
                match item {
                    Ok(item) => ready.push(item),
                    Err(error) => failure = Some(error),
                }
            }
            if rate.latest && ready.len() > 1 {
                ready.drain(..ready.len() - 1);
            }
            if !ready.is_empty() && !publish.send(Ok(ready)).await {
                return;
            }
            if let Some(error) = failure {
                publish.send(Err(error)).await;
                return;
            }
        }
    });
    Batches {
        updates,
        _producer: producer,
        _delivery: delivery,
    }
}

enum Refresh {
    Automatic {
        prefer_cache: bool,
        queued: Arc<AtomicBool>,
    },
    Requested(Sender<Result<(), ServiceError>>),
}

async fn watch_daemon<B: Clients>(
    backend: Arc<B>,
    store: Store,
    inventory: Option<Sender<Refresh>>,
) {
    let queued = Arc::new(AtomicBool::new(false));
    loop {
        if let Ok(mut feed) = backend.daemon().await {
            let mut ready = false;
            let mut generation = String::new();
            while let Ok(Some(event)) = feed.pull().await {
                let new_daemon = generation != event.stream_id;
                if new_daemon {
                    ready = false;
                    generation.clone_from(&event.stream_id);
                }
                let snapshot = matches!(event.body, Some(Body::Snapshot(_)));
                let changed = matches!(event.body, Some(Body::DockerInventoryChanged(_)));
                let engine = match &event.body {
                    Some(Body::Snapshot(snapshot)) => snapshot.engine.as_ref(),
                    Some(Body::Engine(engine)) => Some(engine),
                    _ => None,
                };
                let online = engine.map_or(ready && !snapshot, |engine| {
                    matches!(engine.state(), EngineState::Ready | EngineState::Idle)
                });
                store.apply(event, new_daemon || (ready && !online));
                let prefer_cache = !ready || snapshot;
                if let Some(inventory) = &inventory
                    && online
                    && (prefer_cache || changed)
                    // One queued refresh absorbs any burst while a read is in flight.
                    && !queued.swap(true, Ordering::Relaxed)
                    && !inventory.try_send(Refresh::Automatic {
                        prefer_cache,
                        queued: Arc::clone(&queued),
                    })
                {
                    queued.store(false, Ordering::Relaxed);
                }
                ready = online;
            }
            store.reset();
        }
        delay(RETRY).await;
    }
}

async fn refresh_inventory<B: Clients>(
    backend: Arc<B>,
    store: Store,
    mut requests: Receiver<Refresh>,
) {
    while let Some(request) = requests.next().await {
        let revision = store.docker_revision();
        if let Refresh::Automatic {
            prefer_cache,
            queued,
        } = &request
        {
            queued.store(false, Ordering::Relaxed);
            if *prefer_cache && let Ok(cached) = backend.inventory(true).await {
                store.replace_docker(cached, revision);
            }
        }
        let result = backend.inventory(false).await.map(|snapshot| {
            store.replace_docker(snapshot, revision);
        });
        if let Refresh::Requested(reply) = request {
            reply.send(result).await;
        }
    }
}

trait Feed<T>: Send + 'static {
    fn pull(&mut self) -> impl Future<Output = Result<Option<T>, ServiceError>> + Send;
}

trait Clients: Send + Sync + 'static {
    type Daemon: Feed<DaemonEvent>;
    type Logs: Feed<LogLine>;
    type Stats: Feed<StatsSample>;

    fn daemon(&self) -> impl Future<Output = Result<Self::Daemon, ServiceError>> + Send;
    fn inventory(
        &self,
        prefer_cache: bool,
    ) -> impl Future<Output = Result<ResourceSnapshot, ServiceError>> + Send;
    fn inspect(
        &self,
        id: &ContainerId,
    ) -> impl Future<Output = Result<ContainerDetails, ServiceError>> + Send;
    fn logs(
        &self,
        id: &ContainerId,
    ) -> impl Future<Output = Result<Self::Logs, ServiceError>> + Send;
    fn stats(
        &self,
        id: &ContainerId,
    ) -> impl Future<Output = Result<Self::Stats, ServiceError>> + Send;
    fn dispatch(&self, command: Command) -> impl Future<Output = Result<(), ServiceError>> + Send;
}

impl Feed<ComposeOutput> for ComposeStream {
    async fn pull(&mut self) -> Result<Option<ComposeOutput>, ServiceError> {
        self.next().await.map_err(ServiceError::Compose)
    }
}

struct LocalClients {
    daemon: DaemonEndpoint,
    docker: DockerClient,
}

impl Feed<DaemonEvent> for EventSubscription {
    async fn pull(&mut self) -> Result<Option<DaemonEvent>, ServiceError> {
        Ok(Some(self.next().await?))
    }
}

macro_rules! docker_feed {
    ($feed:ty, $item:ty) => {
        impl Feed<$item> for $feed {
            async fn pull(&mut self) -> Result<Option<$item>, ServiceError> {
                Ok(self.next().await?)
            }
        }
    };
}

docker_feed!(DockerLogs, LogLine);
docker_feed!(DockerStats, StatsSample);
docker_feed!(ImagePull, PullProgress);

impl Clients for LocalClients {
    type Daemon = EventSubscription;
    type Logs = DockerLogs;
    type Stats = DockerStats;

    async fn daemon(&self) -> Result<Self::Daemon, ServiceError> {
        Ok(self.daemon.connect().await?.subscribe_events())
    }

    async fn inventory(&self, prefer_cache: bool) -> Result<ResourceSnapshot, ServiceError> {
        let daemon = self.daemon.connect().await?;
        // Older daemons lack the event contract and the full inventory response bounds.
        if !prefer_cache && daemon.api_minor() < 15 {
            return Ok(ResourceSnapshot {
                containers: self.docker.containers().await?,
                images: self.docker.images().await?,
                volumes: self.docker.volumes().await?,
                networks: self.docker.networks().await?,
            });
        }
        Ok(daemon.docker_inventory(prefer_cache).await?)
    }

    async fn inspect(&self, id: &ContainerId) -> Result<ContainerDetails, ServiceError> {
        Ok(self.docker.inspect(id).await?)
    }

    async fn logs(&self, id: &ContainerId) -> Result<Self::Logs, ServiceError> {
        Ok(self.docker.logs(id).await?)
    }

    async fn stats(&self, id: &ContainerId) -> Result<Self::Stats, ServiceError> {
        Ok(self.docker.stats(id).await?)
    }

    async fn dispatch(&self, command: Command) -> Result<(), ServiceError> {
        match command {
            Command::Engine { action, request_id } => {
                let mut engine = self.daemon.connect().await?.engine();
                let request = EngineActionRequest { request_id };
                match action {
                    EngineAction::Start => engine.start(request).await,
                    EngineAction::Stop => engine.stop(request).await,
                    EngineAction::Restart => engine.restart(request).await,
                }
                .map_err(ClientError::from)?;
            }
            Command::Container { id, action } => self.docker.act(&id, action).await?,
            Command::RemoveContainer(confirmed) => self.docker.remove_container(confirmed).await?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
