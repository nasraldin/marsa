//! Own one stream reader and writer and correlate replies with bounded host work.
//! A reserved shutdown request closes the ordinary queue and follows the current
//! wire request, preserving response correlation until the guest acknowledges.

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
        CommandStatus, EngineHealthState, GuestEnvelope, Shutdown,
        guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::{Vm, VsockStream, async_trait};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout, timeout_at},
};

use super::{
    CONNECT_RETRY, ENGINE_TIMEOUT, GuestControlError, GuestEvent, GuestResponse, REQUEST_TIMEOUT,
    wire,
};
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
        if matches!(body, HostBody::Shutdown(_)) {
            return Err(GuestControlError::Sequence(
                "guest shutdown is owned by the supervisor",
            ));
        }
        let slot = self.queue.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => GuestControlError::Busy,
            mpsc::error::TrySendError::Closed(_) => GuestControlError::Disconnected,
        })?;
        let deadline = Instant::now() + duration;
        let (request, response) = self.prepare(body, deadline)?;
        slot.send(request);
        response_before(deadline, response).await
    }

    fn prepare(
        &self,
        body: HostBody,
        deadline: Instant,
    ) -> Result<(Request, oneshot::Receiver<GuestResponse>), GuestControlError> {
        let id = self
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| GuestControlError::Sequence("request IDs exhausted"))?;
        let trim = matches!(body, HostBody::Trim(_));
        let frame = wire::encode(id, body)?;
        let (reply, response) = oneshot::channel();
        Ok((
            Request {
                id,
                trim,
                deadline,
                frame,
                reply,
            },
            response,
        ))
    }
}

async fn response_before(
    deadline: Instant,
    response: oneshot::Receiver<GuestResponse>,
) -> Result<GuestResponse, GuestControlError> {
    match timeout_at(deadline, response).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) if Instant::now() < deadline => Err(GuestControlError::Disconnected),
        _ => Err(GuestControlError::TimedOut("waiting for a command result")),
    }
}

pub(super) struct Session {
    pub(super) client: Arc<Client>,
    active: watch::Sender<Weak<Client>>,
    shutdown: Option<oneshot::Sender<Request>>,
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
        let (shutdown, stop) = oneshot::channel();
        let (publish_health, health) = watch::channel(None);
        let (publish_failure, failure) = watch::channel(None);
        let worker = tokio::spawn(async move {
            if let Err(error) = exchange(stream, requests, stop, publish_health, events).await {
                publish_failure.send_replace(Some(Arc::new(error)));
            }
        });
        Self {
            client: Arc::new(Client {
                queue,
                next_id: AtomicU64::new(1),
            }),
            active,
            shutdown: Some(shutdown),
            health,
            failure,
            worker,
        }
    }

    fn revoke(&self) {
        let client = Arc::downgrade(&self.client);
        self.active.send_if_modified(|active| {
            if !active.ptr_eq(&client) {
                return false;
            }
            *active = Weak::new();
            true
        });
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

    async fn shutdown(&mut self) -> io::Result<()> {
        self.revoke();
        let send = self
            .shutdown
            .take()
            .ok_or(GuestControlError::Disconnected)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let (request, response) = self
            .client
            .prepare(HostBody::Shutdown(Shutdown {}), deadline)?;
        send.send(request)
            .map_err(|_| GuestControlError::Disconnected)?;
        match response_before(deadline, response).await? {
            GuestResponse::Command(result) if result.status() == CommandStatus::Ok => Ok(()),
            GuestResponse::Command(result) => {
                Err(GuestControlError::ShutdownRejected(result.status()).into())
            }
            GuestResponse::Trim(_) => {
                Err(GuestControlError::Sequence("expected shutdown result").into())
            }
        }
    }

    async fn disconnected(&mut self) -> io::Error {
        Self::failure(&mut self.failure).await
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.revoke();
        self.worker.abort();
    }
}

async fn exchange(
    stream: VsockStream,
    requests: mpsc::Receiver<Request>,
    mut shutdown: oneshot::Receiver<Request>,
    health: watch::Sender<Option<EngineHealthState>>,
    events: broadcast::Sender<GuestEvent>,
) -> Result<(), GuestControlError> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut requests = Some(requests);
    let mut reserved = None;
    let mut current = None::<Request>;
    // The guest runs one command at a time. Keep reading events and retain
    // the in-flight request even when shutdown revokes ordinary command intake.
    loop {
        let incoming = wire::read(&mut reader);
        tokio::pin!(incoming);
        let envelope = loop {
            let request = if current.is_none() && reserved.is_some() {
                reserved.take()
            } else {
                let deadline = current.as_ref().map(|request| request.deadline);
                tokio::select! {
                    biased;
                    stop = &mut shutdown, if requests.is_some() => {
                        // Drop queued commands and reject stale clients that still hold
                        // a reference to this session. The reserved request is next.
                        requests = None;
                        let Ok(stop) = stop else { return Ok(()); };
                        reserved = Some(stop);
                        continue;
                    }
                    () = wait_deadline(deadline) => {
                        // Closing prevents a late response from matching later work.
                        return Err(GuestControlError::TimedOut("waiting for a command result"));
                    }
                    envelope = &mut incoming => break envelope?,
                    request = receive(&mut requests), if current.is_none() => request,
                }
            };
            let Some(request) = request else {
                return Ok(());
            };
            if request.reply.is_closed() || request.deadline <= Instant::now() {
                continue;
            }
            wire::write(&mut writer, &request.frame).await?;
            current = Some(request);
        };
        accept(envelope, &mut current, &health, &events)?;
    }
}

async fn receive(requests: &mut Option<mpsc::Receiver<Request>>) -> Option<Request> {
    match requests {
        Some(requests) => requests.recv().await,
        None => pending().await,
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
