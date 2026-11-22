//! Own one stream reader and writer and correlate replies with bounded host work.
//! A reserved shutdown request closes the ordinary queue and follows the current
//! wire request, preserving response correlation until the guest acknowledges.

use std::{
    future::pending,
    io,
    net::Ipv4Addr,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use marsa_proto::guest::{
    ProtocolError,
    v1::{
        CommandStatus, EgressStage, EngineHealth, EngineHealthState, GuestEnvelope, ProbeEgress,
        Shutdown, StartEngine, guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::{Vm, VsockStream, async_trait};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout, timeout_at},
};

use super::{
    CONNECT_RETRY, ENGINE_TIMEOUT, GuestControlError, GuestEvent, GuestResponse, REQUEST_TIMEOUT,
    observations::{EventBus, SessionEvents},
    wire,
};
use crate::supervisor::GuestSession;

pub(super) const REQUEST_CAPACITY: usize = 8;

struct Request {
    id: u64,
    result: ResultType,
    deadline: Instant,
    frame: Vec<u8>,
    reply: oneshot::Sender<GuestResponse>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultType {
    Command,
    Trim,
    Egress,
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
        let result = match body {
            HostBody::Trim(_) => ResultType::Trim,
            HostBody::ProbeEgress(_) => ResultType::Egress,
            _ => ResultType::Command,
        };
        let frame = wire::encode(id, body)?;
        let (reply, response) = oneshot::channel();
        Ok((
            Request {
                id,
                result,
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
    bind_worker: Option<JoinHandle<()>>,
    events: SessionEvents,
    deferred: bool,
}

impl Session {
    pub(super) fn start(
        stream: VsockStream,
        active: watch::Sender<Weak<Client>>,
        events: EventBus,
        deferred: bool,
    ) -> Self {
        let (queue, requests) = mpsc::channel(REQUEST_CAPACITY);
        let (shutdown, stop) = oneshot::channel();
        let (publish_health, health) = watch::channel(None);
        let (publish_failure, failure) = watch::channel(None);
        let client = Arc::new(Client {
            queue,
            next_id: AtomicU64::new(1),
        });
        let events = events.session(active.clone(), Arc::downgrade(&client));
        let publish = events.clone();
        let worker = tokio::spawn(async move {
            if let Err(error) = exchange(stream, requests, stop, publish_health, publish).await {
                publish_failure.send_replace(Some(Arc::new(error)));
            }
        });
        Self {
            client,
            active,
            shutdown: Some(shutdown),
            health,
            failure,
            worker,
            bind_worker: None,
            events,
            deferred,
        }
    }

    pub(super) fn retain_bind_worker(&mut self, worker: JoinHandle<()>) {
        self.bind_worker = Some(worker);
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
    fn can_probe_egress(&self) -> bool {
        self.deferred
    }

    async fn probe_egress(&mut self, target: Ipv4Addr) -> io::Result<EgressStage> {
        let response = self
            .client
            .request(
                HostBody::ProbeEgress(ProbeEgress {
                    address_v4: target.octets().to_vec(),
                }),
                Duration::from_secs(12),
            )
            .await?;
        match response {
            GuestResponse::Egress(stage) => Ok(stage),
            _ => Err(GuestControlError::Sequence("expected egress result").into()),
        }
    }

    async fn engine_ready(&mut self, vm: &dyn Vm) -> io::Result<()> {
        if self.deferred {
            match self
                .client
                .request(HostBody::StartEngine(StartEngine {}), REQUEST_TIMEOUT)
                .await?
            {
                GuestResponse::Command(result) if result.status() == CommandStatus::Ok => {
                    self.deferred = false;
                }
                _ => {
                    return Err(GuestControlError::Sequence("guest rejected engine startup").into());
                }
            }
        }
        tokio::select! {
            biased;
            error = Self::failure(&mut self.failure) => return Err(error),
            result = timeout(ENGINE_TIMEOUT, Self::verify_engine(vm, &mut self.health)) => {
                result.map_err(|_| GuestControlError::TimedOut("waiting for Docker readiness"))??;
            }
        }
        self.active.send_replace(Arc::downgrade(&self.client));
        if let Some(state) = *self.health.borrow() {
            self.events
                .publish_active(&GuestEvent::Health(EngineHealth {
                    state: state.into(),
                    ..Default::default()
                }));
        }
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
            GuestResponse::Trim(_) | GuestResponse::Egress(_) => {
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
        if let Some(worker) = &self.bind_worker {
            worker.abort();
        }
    }
}

async fn exchange(
    stream: VsockStream,
    requests: mpsc::Receiver<Request>,
    mut shutdown: oneshot::Receiver<Request>,
    health: watch::Sender<Option<EngineHealthState>>,
    events: SessionEvents,
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
    events: &SessionEvents,
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
            match request.result {
                ResultType::Egress => {
                    return Err(GuestControlError::Sequence(
                        "egress probe requires EgressResult",
                    ));
                }
                ResultType::Trim if result.status() == CommandStatus::Ok => {
                    return Err(GuestControlError::Sequence(
                        "trim success requires TrimResult",
                    ));
                }
                _ => {}
            }
            let _ = request.reply.send(GuestResponse::Command(result));
            return Ok(());
        }
        Some(GuestBody::TrimResult(result)) => {
            let request = matching_request(current, envelope.request_id)?;
            if request.result != ResultType::Trim {
                return Err(GuestControlError::Sequence("unexpected TrimResult"));
            }
            let _ = request.reply.send(GuestResponse::Trim(result));
            return Ok(());
        }
        Some(GuestBody::EgressResult(result)) => {
            let request = matching_request(current, envelope.request_id)?;
            if request.result != ResultType::Egress || result.stage() == EgressStage::Unspecified {
                return Err(GuestControlError::Sequence("unexpected egress result"));
            }
            let _ = request.reply.send(GuestResponse::Egress(result.stage()));
            return Ok(());
        }
        Some(GuestBody::Hello(_)) => return Err(GuestControlError::Sequence("unexpected Hello")),
        None => return Err(ProtocolError::InvalidField("body").into()),
    };
    events.publish(event);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn only_the_current_ready_session_can_publish_to_the_daemon() {
        let client = || {
            Arc::new(Client {
                queue: mpsc::channel(1).0,
                next_id: AtomicU64::new(1),
            })
        };
        let first = client();
        let second = client();
        let (active, _) = watch::channel(Weak::new());
        let delivered = Arc::new(AtomicUsize::new(0));
        let counter = delivered.clone();
        let mut bus = EventBus::default();
        bus.observer = Some(Arc::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        let old = bus.clone().session(active.clone(), Arc::downgrade(&first));
        let new = bus.session(active.clone(), Arc::downgrade(&second));
        let sample = || GuestEvent::Stats(Default::default());

        old.publish(sample());
        assert_eq!(delivered.load(Ordering::Relaxed), 0);
        active.send_replace(Arc::downgrade(&first));
        old.publish(sample());
        new.publish(sample());
        assert_eq!(delivered.load(Ordering::Relaxed), 1);
        active.send_replace(Arc::downgrade(&second));
        old.publish(sample());
        new.publish(sample());
        assert_eq!(delivered.load(Ordering::Relaxed), 2);
    }
}
