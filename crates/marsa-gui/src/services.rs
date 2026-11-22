//! Keep daemon, Docker and selected resource streams scoped to the desktop window.

use std::{error::Error, fmt, future::Future, io, sync::Arc, time::Duration};

use marsa_client::{
    ClientError, DaemonEndpoint, EventSubscription,
    api::{DaemonEvent, EngineActionRequest, EngineState, daemon_event::Body},
    docker::{
        ContainerAction, ContainerDetails, ContainerId, ContainerSummary, DockerClient,
        DockerError, DockerLogs, DockerStats, ImageSummary, LogLine, NetworkSummary, StatsSample,
        VolumeSummary,
    },
    runtime::{Background, Receiver, Spawner, Task, delay, queue},
};

use crate::store::{ResourceSnapshot, Store};

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
    /// The background runtime could not start.
    Runtime(io::Error),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daemon(error) => fmt::Display::fmt(error, f),
            Self::Docker(error) => fmt::Display::fmt(error, f),
            Self::Runtime(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            Self::Daemon(error) => error,
            Self::Docker(error) => error,
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
        let clients = LocalClients {
            daemon: DaemonEndpoint::discover()?,
            docker: DockerClient::discover()?,
        };
        Ok(Self {
            session: Session::start(clients)?,
        })
    }

    /// Read normalized state through the window's store.
    pub fn store(&self) -> &Store {
        &self.session.store
    }

    /// Dispatch a command on the worker and receive its completion separately from state updates.
    pub fn dispatch(&self, command: Command) -> PendingCommand {
        self.session.dispatch(command)
    }

    /// Refresh the container and image lists from the local engine on demand.
    pub fn refresh(&self) -> PendingCommand {
        let backend = Arc::clone(&self.session.backend);
        let store = self.session.store.clone();
        let (reply, completion) = queue(1);
        let task = self.session.runtime.spawn(async move {
            reply.send(snapshot(&*backend, &store).await).await;
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

    /// Follow a container's log output in ordered, bounded batches.
    pub fn logs(&self, id: ContainerId) -> Batches<LogLine> {
        self.session.logs(id)
    }

    /// Follow the latest container stats sample at a capped display rate.
    pub fn stats(&self, id: ContainerId) -> Batches<StatsSample> {
        self.session.stats(id)
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
    runtime: Background,
}

impl<B: Clients> Session<B> {
    fn start(backend: B) -> Result<Self, ServiceError> {
        let runtime = Background::new().map_err(ServiceError::Runtime)?;
        let backend = Arc::new(backend);
        let store = Store::default();
        let daemon = runtime.spawn(watch_daemon(
            Arc::clone(&backend),
            store.clone(),
            runtime.spawner(),
        ));
        Ok(Self {
            backend,
            store,
            _daemon: daemon,
            runtime,
        })
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
        batch_stream(
            &self.runtime,
            async move { backend.logs(&id).await },
            Rate {
                interval: LOG_RATE,
                maximum: LOG_LINES,
                budget: LOG_BYTES,
                latest: false,
            },
            |line| line.text.len(),
        )
    }

    fn stats(&self, id: ContainerId) -> Batches<StatsSample> {
        let backend = Arc::clone(&self.backend);
        batch_stream(
            &self.runtime,
            async move { backend.stats(&id).await },
            Rate {
                interval: STATS_RATE,
                maximum: 64,
                budget: usize::MAX,
                latest: true,
            },
            |_| 1,
        )
    }
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

async fn watch_daemon<B: Clients>(backend: Arc<B>, store: Store, spawner: Spawner) {
    loop {
        if let Ok(mut feed) = backend.daemon().await {
            let mut docker = None;
            let mut generation = String::new();
            let mut ready = false;
            while let Ok(Some(event)) = feed.pull().await {
                if generation != event.stream_id {
                    docker = None;
                    ready = false;
                    generation.clone_from(&event.stream_id);
                    store.replace_docker(ResourceSnapshot::default());
                }
                let engine = match &event.body {
                    Some(Body::Snapshot(snapshot)) => snapshot.engine.as_ref(),
                    Some(Body::Engine(engine)) => Some(engine),
                    _ => None,
                };
                if let Some(engine) = engine {
                    let online = matches!(engine.state(), EngineState::Ready | EngineState::Idle);
                    if online && !ready {
                        let backend = Arc::clone(&backend);
                        let store = store.clone();
                        // Docker's public socket holds an activity lease. Take
                        // a bounded snapshot here without keeping it open.
                        docker = Some(spawner.spawn(async move {
                            let _ = snapshot(&*backend, &store).await;
                        }));
                    } else if !online && ready {
                        docker = None;
                        store.replace_docker(ResourceSnapshot::default());
                    }
                    ready = online;
                }
                store.apply(event);
            }
            drop(docker);
            store.reset();
        }
        delay(RETRY).await;
    }
}

async fn snapshot<B: Clients>(backend: &B, store: &Store) -> Result<(), ServiceError> {
    let snapshot = ResourceSnapshot {
        containers: backend.containers().await?,
        images: backend.images().await?,
        volumes: backend.volumes().await?,
        networks: backend.networks().await?,
    };
    store.replace_docker(snapshot);
    Ok(())
}

trait Feed<T>: Send + 'static {
    fn pull(&mut self) -> impl Future<Output = Result<Option<T>, ServiceError>> + Send;
}

trait Clients: Send + Sync + 'static {
    type Daemon: Feed<DaemonEvent>;
    type Logs: Feed<LogLine>;
    type Stats: Feed<StatsSample>;

    fn daemon(&self) -> impl Future<Output = Result<Self::Daemon, ServiceError>> + Send;
    fn containers(
        &self,
    ) -> impl Future<Output = Result<Vec<ContainerSummary>, ServiceError>> + Send;
    fn images(&self) -> impl Future<Output = Result<Vec<ImageSummary>, ServiceError>> + Send;
    fn volumes(&self) -> impl Future<Output = Result<Vec<VolumeSummary>, ServiceError>> + Send;
    fn networks(&self) -> impl Future<Output = Result<Vec<NetworkSummary>, ServiceError>> + Send;
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

impl Clients for LocalClients {
    type Daemon = EventSubscription;
    type Logs = DockerLogs;
    type Stats = DockerStats;

    async fn daemon(&self) -> Result<Self::Daemon, ServiceError> {
        Ok(self.daemon.connect().await?.subscribe_events())
    }

    async fn containers(&self) -> Result<Vec<ContainerSummary>, ServiceError> {
        Ok(self.docker.containers().await?)
    }

    async fn images(&self) -> Result<Vec<ImageSummary>, ServiceError> {
        Ok(self.docker.images().await?)
    }

    async fn volumes(&self) -> Result<Vec<VolumeSummary>, ServiceError> {
        Ok(self.docker.volumes().await?)
    }

    async fn networks(&self) -> Result<Vec<NetworkSummary>, ServiceError> {
        Ok(self.docker.networks().await?)
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
        }
        Ok(())
    }
}
