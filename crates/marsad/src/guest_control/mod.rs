//! Negotiate the guest control stream and retain it for one running VM session.

use std::{
    io,
    sync::{Arc, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use marsa_proto::guest::{
    CONTROL_PORT, DEFER_ENGINE_START_MINOR, ProtocolError, negotiate,
    v1::{
        CommandResult, CommandStatus, ConfigureNetwork, EgressStage, EngineHealth, HelloAck,
        PortEvent, SetTime, TrimResult, VmStats, guest_envelope::Body as GuestBody,
        host_envelope::Body as HostBody,
    },
};
use marsa_vm::{Vm, VmError, async_trait};
use thiserror::Error;
use tokio::{
    sync::{broadcast, watch},
    time::timeout,
};

use crate::supervisor::{GuestProbe, GuestSession};

mod bind_authorization;
mod observations;
mod session;
mod wire;
mod workloads;

#[cfg(test)]
mod tests;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
const ENGINE_TIMEOUT: Duration = Duration::from_secs(45);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(35);
const CONNECT_RETRY: Duration = Duration::from_millis(100);
const EVENT_CAPACITY: usize = 64;

/// Validated messages sent independently of host requests.
#[derive(Debug, Clone)]
pub enum GuestEvent {
    /// An engine-published port changed.
    Port(PortEvent),
    /// The guest's Docker health changed.
    Health(EngineHealth),
    /// Periodic resource usage since the current guest boot.
    Stats(VmStats),
}

/// Validated result matched to a host request and its expected result type.
#[derive(Debug, Clone)]
pub enum GuestResponse {
    /// Command status, including a rejected trim.
    Command(CommandResult),
    /// Successful data filesystem trim.
    Trim(TrimResult),
    /// Validated stage of the bounded guest HTTPS check.
    Egress(EgressStage),
}

/// Failures reported while negotiating or using a guest control session.
#[derive(Debug, Error)]
pub enum GuestControlError {
    /// The guest has no usable control session.
    #[error("guest control is disconnected")]
    Disconnected,
    /// The bounded command queue is full.
    #[error("guest control request queue is full")]
    Busy,
    /// A control operation or partial frame exceeded its deadline.
    #[error("guest control timed out while {0}")]
    TimedOut(&'static str),
    /// The control stream encountered a transport error.
    #[error("guest control I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The hypervisor could not open the requested transport.
    #[error(transparent)]
    Vm(#[from] VmError),
    /// A frame or version violated the shared guest protocol.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// A valid message arrived in an invalid session state.
    #[error("guest control protocol violation: {0}")]
    Sequence(&'static str),
    /// Initial guest clock setup was rejected.
    #[error("guest rejected initial clock setup ({0:?})")]
    ClockRejected(CommandStatus),
    /// The running guest rejected orderly shutdown.
    #[error("guest rejected shutdown ({0:?})")]
    ShutdownRejected(CommandStatus),
    /// The host clock could not be represented as a guest time.
    #[error("host clock cannot be represented as Unix epoch time")]
    InvalidClock,
}

impl GuestControlError {
    fn io_kind(&self) -> io::ErrorKind {
        match self {
            Self::TimedOut(_) => io::ErrorKind::TimedOut,
            Self::Protocol(_) | Self::Sequence(_) => io::ErrorKind::InvalidData,
            Self::Io(error) => error.kind(),
            Self::Disconnected => io::ErrorKind::NotConnected,
            _ => io::ErrorKind::Other,
        }
    }
}

impl From<GuestControlError> for io::Error {
    fn from(error: GuestControlError) -> Self {
        Self::new(error.io_kind(), error)
    }
}

/// Guest control probe and host command handle for one supervisor.
#[derive(Clone)]
pub struct GuestControl {
    active: watch::Sender<Weak<session::Client>>,
    events: observations::EventBus,
    clock: fn() -> SystemTime,
    agent_connect_timeout: Duration,
    bind_policy: Option<Arc<dyn BindPolicy>>,
}

#[async_trait]
pub(crate) trait BindPolicy: Send + Sync {
    async fn authorize(&self, sources: Vec<String>) -> Option<&'static str>;
}

impl Default for GuestControl {
    fn default() -> Self {
        Self {
            active: watch::channel(Weak::new()).0,
            events: observations::EventBus::default(),
            clock: SystemTime::now,
            agent_connect_timeout: AGENT_CONNECT_TIMEOUT,
            bind_policy: None,
        }
    }
}

impl GuestControl {
    pub(crate) fn authorize_binds(mut self, policy: impl BindPolicy + 'static) -> Self {
        self.bind_policy = Some(Arc::new(policy));
        self
    }

    pub(crate) fn observe_active(
        mut self,
        observer: impl Fn(&GuestEvent) + Send + Sync + 'static,
    ) -> Self {
        self.events.observer = Some(Arc::new(observer));
        self
    }

    /// Set the deadline for the guest agent's listener to appear.
    ///
    /// The packaged runtime allows for first-boot disk initialization. Frame
    /// and negotiation deadlines continue to apply after the connection opens.
    pub fn with_agent_connect_timeout(mut self, limit: Duration) -> Self {
        self.agent_connect_timeout = limit;
        self
    }

    /// Subscribe to future guest events across reconnects. Lagging receivers
    /// receive a broadcast lag notice and must refresh their view.
    pub fn events(&self) -> broadcast::Receiver<GuestEvent> {
        self.events.subscribe()
    }

    /// Submit a command after the supervisor establishes engine readiness.
    /// The queue and response wait are bounded. The host manages negotiation.
    pub async fn request(&self, body: HostBody) -> Result<GuestResponse, GuestControlError> {
        let client = self
            .active
            .borrow()
            .upgrade()
            .ok_or(GuestControlError::Disconnected)?;
        if matches!(body, HostBody::StartEngine(_) | HostBody::ProbeEgress(_)) {
            return Err(GuestControlError::Sequence(
                "boot commands are owned by the supervisor",
            ));
        }
        client.request(body, REQUEST_TIMEOUT).await
    }

    async fn connect(
        &self,
        mut stream: marsa_vm::VsockStream,
        policy: Option<&ConfigureNetwork>,
    ) -> io::Result<session::Session> {
        let message = wire::read(&mut stream).await?;
        let Some(GuestBody::Hello(hello)) = message.body else {
            return Err(GuestControlError::Sequence("expected initial Hello").into());
        };
        let version = negotiate(
            hello
                .protocol
                .ok_or(ProtocolError::InvalidField("protocol"))
                .map_err(GuestControlError::from)?,
        )
        .map_err(GuestControlError::from)?;
        let deferred = version.minor >= DEFER_ENGINE_START_MINOR;
        if policy.is_some() && !deferred {
            // A legacy guest releases the engine when it accepts the clock.
            return Err(io::ErrorKind::Unsupported.into());
        }
        wire::write(
            &mut stream,
            &wire::encode(
                0,
                HostBody::HelloAck(HelloAck {
                    protocol: Some(version),
                    defer_engine_start: deferred,
                }),
            )?,
        )
        .await?;

        let session =
            session::Session::start(stream, self.active.clone(), self.events.clone(), deferred);
        let now = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| GuestControlError::InvalidClock)?;
        let response = session
            .client
            .request(
                HostBody::SetTime(SetTime {
                    unix_seconds: i64::try_from(now.as_secs())
                        .map_err(|_| GuestControlError::InvalidClock)?,
                    nanoseconds: now.subsec_nanos(),
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
        match response {
            GuestResponse::Command(result) if result.status() == CommandStatus::Ok => (),
            GuestResponse::Command(result) => {
                return Err(GuestControlError::ClockRejected(result.status()).into());
            }
            GuestResponse::Trim(_) | GuestResponse::Egress(_) => {
                return Err(GuestControlError::Sequence("expected clock setup result").into());
            }
        }
        if let Some(policy) = policy {
            let response = session
                .client
                .request(
                    HostBody::ConfigureNetwork(policy.clone()),
                    HANDSHAKE_TIMEOUT,
                )
                .await?;
            if !matches!(response, GuestResponse::Command(result) if result.status() == CommandStatus::Ok)
            {
                // The guest's reply text can include sensitive configuration.
                return Err(
                    GuestControlError::Sequence("guest rejected boot network policy").into(),
                );
            }
        }
        Ok(session)
    }
}

#[async_trait]
impl GuestProbe for GuestControl {
    async fn active_containers(&self, vm: &dyn Vm) -> Option<io::Result<u32>> {
        Some(workloads::count(vm).await)
    }

    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        self.handshake_for_boot(vm, None).await
    }

    async fn handshake_for_boot(
        &self,
        vm: &dyn Vm,
        policy: Option<&ConfigureNetwork>,
    ) -> io::Result<Box<dyn GuestSession>> {
        let stream = timeout(self.agent_connect_timeout, wire::open(vm, CONTROL_PORT))
            .await
            .map_err(|_| GuestControlError::TimedOut("connecting to the guest agent"))??;
        let mut session = timeout(HANDSHAKE_TIMEOUT, self.connect(stream, policy))
            .await
            .map_err(|_| GuestControlError::TimedOut("negotiating the guest session"))??;
        if let Some(policy) = &self.bind_policy {
            session.retain_bind_worker(bind_authorization::connect(vm, policy.clone()).await?);
        }
        Ok(Box::new(session))
    }
}
