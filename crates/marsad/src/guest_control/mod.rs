//! Negotiate the guest control stream and retain it for one running VM session.

use std::{
    io,
    sync::Weak,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use marsa_proto::guest::{
    CONTROL_PORT, ProtocolError, negotiate,
    v1::{
        CommandResult, CommandStatus, EngineHealth, HelloAck, PortEvent, SetTime, TrimResult,
        VmStats, guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::{Vm, VmError, async_trait};
use thiserror::Error;
use tokio::{
    sync::{broadcast, watch},
    time::timeout,
};

use crate::supervisor::{GuestProbe, GuestSession};

mod session;
mod wire;

#[cfg(test)]
mod tests;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
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
    events: broadcast::Sender<GuestEvent>,
    clock: fn() -> SystemTime,
}

impl Default for GuestControl {
    fn default() -> Self {
        Self {
            active: watch::channel(Weak::new()).0,
            events: broadcast::channel(EVENT_CAPACITY).0,
            clock: SystemTime::now,
        }
    }
}

impl GuestControl {
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
        client.request(body, REQUEST_TIMEOUT).await
    }

    async fn connect(&self, vm: &dyn Vm) -> Result<session::Session, GuestControlError> {
        let mut stream = wire::open(vm, CONTROL_PORT).await?;
        let message = wire::read(&mut stream).await?;
        let Some(GuestBody::Hello(hello)) = message.body else {
            return Err(GuestControlError::Sequence("expected initial Hello"));
        };
        let version = negotiate(
            hello
                .protocol
                .ok_or(ProtocolError::InvalidField("protocol"))?,
        )?;
        wire::write(
            &mut stream,
            &wire::encode(
                0,
                HostBody::HelloAck(HelloAck {
                    protocol: Some(version),
                }),
            )?,
        )
        .await?;

        let session = session::Session::start(stream, self.active.clone(), self.events.clone());
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
            GuestResponse::Command(result) if result.status() == CommandStatus::Ok => Ok(session),
            GuestResponse::Command(result) => {
                Err(GuestControlError::ClockRejected(result.status()))
            }
            GuestResponse::Trim(_) => {
                Err(GuestControlError::Sequence("expected clock setup result"))
            }
        }
    }
}

#[async_trait]
impl GuestProbe for GuestControl {
    async fn handshake(&self, vm: &dyn Vm) -> io::Result<Box<dyn GuestSession>> {
        let session = timeout(HANDSHAKE_TIMEOUT, self.connect(vm))
            .await
            .map_err(|_| GuestControlError::TimedOut("negotiating the guest session"))??;
        Ok(Box::new(session))
    }
}
