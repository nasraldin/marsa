//! Own one stream reader and writer and correlate replies with bounded host work.

use std::{
    future::pending,
    io,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use marsa_proto::guest::{
    ProtocolError,
    v1::{
        CommandStatus, EngineHealthState, GuestEnvelope, guest_envelope::Body as GuestBody,
        host_envelope::Body as HostBody,
    },
};
use marsa_vm::{Vm, VsockStream, async_trait};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout, timeout_at},
};

use super::{CONNECT_RETRY, ENGINE_TIMEOUT, GuestControlError, GuestEvent, GuestResponse, wire};
use crate::supervisor::GuestSession;

pub(super) const REQUEST_CAPACITY: usize = 8;

struct Request {
    id: u64,
    trim: bool,
    deadline: Instant,
    frame: Vec<u8>,
    reply: oneshot::Sender<GuestResponse>,
}

pub(super) struct Client {
    queue: mpsc::Sender<Request>,
    next_id: AtomicU64,
}

impl Client {
    pub(super) async fn request(
        &self,
        body: HostBody,
        duration: Duration,
    ) -> Result<GuestResponse, GuestControlError> {
        let slot = self.queue.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => GuestControlError::Busy,
            mpsc::error::TrySendError::Closed(_) => GuestControlError::Disconnected,
        })?;
        let deadline = Instant::now() + duration;
        let id = self
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| GuestControlError::Sequence("request IDs exhausted"))?;
        let trim = matches!(body, HostBody::Trim(_));
        let frame = wire::encode(id, body)?;
        let (reply, response) = oneshot::channel();
        slot.send(Request {
            id,
            trim,
            deadline,
            frame,
            reply,
        });
        match timeout_at(deadline, response).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) if Instant::now() < deadline => Err(GuestControlError::Disconnected),
            _ => Err(GuestControlError::TimedOut("waiting for a command result")),
        }
    }
}

pub(super) struct Session {
    pub(super) client: Arc<Client>,
    active: watch::Sender<Weak<Client>>,
    health: watch::Receiver<Option<EngineHealthState>>,
    failure: watch::Receiver<Option<Arc<GuestControlError>>>,
    worker: JoinHandle<()>,
}

impl Session {
    pub(super) fn start(
        stream: VsockStream,
        active: watch::Sender<Weak<Client>>,
        events: broadcast::Sender<GuestEvent>,
    ) -> Self {
        let (queue, requests) = mpsc::channel(REQUEST_CAPACITY);
        let (publish_health, health) = watch::channel(None);
        let (publish_failure, failure) = watch::channel(None);
        let worker = tokio::spawn(async move {
            if let Err(error) = exchange(stream, requests, publish_health, events).await {
                publish_failure.send_replace(Some(Arc::new(error)));
            }
        });
        Self {
            client: Arc::new(Client {
                queue,
                next_id: AtomicU64::new(1),
            }),
            active,
            health,
            failure,
            worker,
        }
    }

    async fn verify_engine(
        vm: &dyn Vm,
        health: &mut watch::Receiver<Option<EngineHealthState>>,
    ) -> Result<(), GuestControlError> {
        loop {
            health
                .wait_for(|state| *state == Some(EngineHealthState::Ready))
                .await
                .map_err(|_| GuestControlError::Disconnected)?;
            if wire::docker_ping(vm).await? && *health.borrow() == Some(EngineHealthState::Ready) {
                return Ok(());
            }
            sleep(CONNECT_RETRY).await;
        }
    }

    async fn failure(failure: &mut watch::Receiver<Option<Arc<GuestControlError>>>) -> io::Error {
        // The connection worker publishes the error before dropping its sender.
        let _ = failure.wait_for(Option::is_some).await;
        match failure.borrow().clone() {
            Some(error) => io::Error::new(error.io_kind(), error),
            None => GuestControlError::Disconnected.into(),
        }
    }
}

#[async_trait]
impl GuestSession for Session {
    async fn engine_ready(&mut self, vm: &dyn Vm) -> io::Result<()> {
        tokio::select! {
            biased;
            error = Self::failure(&mut self.failure) => return Err(error),
            result = timeout(ENGINE_TIMEOUT, Self::verify_engine(vm, &mut self.health)) => {
                result.map_err(|_| GuestControlError::TimedOut("waiting for Docker readiness"))??;
            }
        }
        self.active.send_replace(Arc::downgrade(&self.client));
        Ok(())
    }

    async fn disconnected(&mut self) -> io::Error {
        Self::failure(&mut self.failure).await
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let client = Arc::downgrade(&self.client);
        self.active.send_if_modified(|active| {
            if !active.ptr_eq(&client) {
                return false;
            }
            *active = Weak::new();
            true
        });
        self.worker.abort();
    }
}

async fn exchange(
    stream: VsockStream,
    mut requests: mpsc::Receiver<Request>,
    health: watch::Sender<Option<EngineHealthState>>,
    events: broadcast::Sender<GuestEvent>,
) -> Result<(), GuestControlError> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut current = None::<Request>;
    // The guest runs one command at a time. Keep a single in-flight request
    // and keep reading events and responses while it executes.
    loop {
        let incoming = wire::read(&mut reader);
        tokio::pin!(incoming);
        let envelope = loop {
            let deadline = current.as_ref().map(|request| request.deadline);
            tokio::select! {
                envelope = &mut incoming => break envelope?,
                request = requests.recv(), if current.is_none() => {
                    let Some(request) = request else { return Ok(()); };
                    if request.reply.is_closed() || request.deadline <= Instant::now() {
                        continue;
                    }
                    wire::write(&mut writer, &request.frame).await?;
                    current = Some(request);
                }
                () = wait_deadline(deadline) => {
                    // Closing prevents a late response from being associated
                    // with later work.
                    return Err(GuestControlError::TimedOut("waiting for a command result"));
                }
            }
        };
        accept(envelope, &mut current, &health, &events)?;
    }
}

async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

fn accept(
    envelope: GuestEnvelope,
    current: &mut Option<Request>,
    health: &watch::Sender<Option<EngineHealthState>>,
    events: &broadcast::Sender<GuestEvent>,
) -> Result<(), GuestControlError> {
    let event = match envelope.body {
        Some(GuestBody::PortEvent(event)) => GuestEvent::Port(event),
        Some(GuestBody::EngineHealth(event)) => {
            health.send_replace(Some(event.state()));
            GuestEvent::Health(event)
        }
        Some(GuestBody::Stats(event)) => GuestEvent::Stats(event),
        Some(GuestBody::CommandResult(result)) => {
            let request = matching_request(current, envelope.request_id)?;
            if request.trim && result.status() == CommandStatus::Ok {
                return Err(GuestControlError::Sequence(
                    "trim success requires TrimResult",
                ));
            }
            let _ = request.reply.send(GuestResponse::Command(result));
            return Ok(());
        }
        Some(GuestBody::TrimResult(result)) => {
            let request = matching_request(current, envelope.request_id)?;
            if !request.trim {
                return Err(GuestControlError::Sequence("unexpected TrimResult"));
            }
            let _ = request.reply.send(GuestResponse::Trim(result));
            return Ok(());
        }
        Some(GuestBody::Hello(_)) => return Err(GuestControlError::Sequence("unexpected Hello")),
        None => return Err(ProtocolError::InvalidField("body").into()),
    };
    let _ = events.send(event);
    Ok(())
}

fn matching_request(current: &mut Option<Request>, id: u64) -> Result<Request, GuestControlError> {
    let request =
        current
            .take()
            .filter(|request| request.id == id)
            .ok_or(GuestControlError::Sequence(
                "unexpected response request ID",
            ))?;
    if request.deadline <= Instant::now() {
        return Err(GuestControlError::TimedOut("waiting for a command result"));
    }
    Ok(request)
}
