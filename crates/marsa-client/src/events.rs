//! Reconnect event subscriptions using the daemon's snapshot and stream cursor.

use tokio::time::{sleep, timeout};
use tonic::{Code, Status, Streaming, transport::Channel};

use crate::{
    ClientError, DaemonEndpoint,
    api::{DaemonEvent, SubscribeRequest, daemon_event::Body, events_client::EventsClient},
    client::{RETRY_DELAY, RPC_DEADLINE, STARTUP_DEADLINE, peer_disconnected},
};

/// An ordered subscription that replaces state with a fresh snapshot after a restart.
///
/// An unexpected stream end, temporary transport error or lagged connection
/// triggers another version check and subscription. A snapshot can replace any
/// changes missed during the disconnect.
pub struct EventSubscription {
    endpoint: DaemonEndpoint,
    initial: Option<EventsClient<Channel>>,
    stream: Option<Streaming<DaemonEvent>>,
    cursor: Option<(String, u64)>,
}

impl EventSubscription {
    pub(super) fn new(endpoint: DaemonEndpoint, initial: EventsClient<Channel>) -> Self {
        Self {
            endpoint,
            initial: Some(initial),
            stream: None,
            cursor: None,
        }
    }

    /// Read the next snapshot or live event, reconnecting when the stream is interrupted.
    ///
    /// Apply a snapshot as a replacement for local state. An unchanged cursor on a
    /// resubscription is suppressed. All delivered live events are contiguous.
    pub async fn next(&mut self) -> Result<DaemonEvent, ClientError> {
        loop {
            if self.stream.is_none() {
                let snapshot = timeout(STARTUP_DEADLINE, self.establish())
                    .await
                    .map_err(|_| ClientError::StartupTimeout)??;
                if let Some(event) = snapshot {
                    return Ok(event);
                }
            }
            let Some(stream) = &mut self.stream else {
                continue;
            };
            match stream.message().await {
                Ok(Some(event)) => match self.accept(event, false) {
                    Ok(Some(event)) => return Ok(event),
                    Ok(None) => {}
                    Err(error) => {
                        self.stream = None;
                        return Err(error);
                    }
                },
                Ok(None) => self.reconnect().await,
                Err(status) if transient(&status) => self.reconnect().await,
                Err(status) => {
                    self.stream = None;
                    return Err(status.into());
                }
            }
        }
    }

    async fn establish(&mut self) -> Result<Option<DaemonEvent>, ClientError> {
        loop {
            let mut client = match self.initial.take() {
                Some(client) => client,
                None => self.endpoint.connect().await?.events(),
            };
            match timeout(RPC_DEADLINE, client.subscribe(SubscribeRequest {})).await {
                Ok(Ok(response)) => {
                    let mut stream = response.into_inner();
                    match timeout(RPC_DEADLINE, stream.message()).await {
                        Ok(Ok(Some(event))) => {
                            let snapshot = self.accept(event, true)?;
                            self.stream = Some(stream);
                            return Ok(snapshot);
                        }
                        Ok(Err(status)) if !transient(&status) => return Err(status.into()),
                        _ => {}
                    }
                }
                Ok(Err(status)) if !transient(&status) => return Err(status.into()),
                _ => {}
            }
            sleep(RETRY_DELAY).await;
        }
    }

    async fn reconnect(&mut self) {
        self.stream = None;
        sleep(RETRY_DELAY).await;
    }

    fn accept(
        &mut self,
        event: DaemonEvent,
        expect_snapshot: bool,
    ) -> Result<Option<DaemonEvent>, ClientError> {
        let snapshot = matches!(&event.body, Some(Body::Snapshot(_)));
        if event.stream_id.is_empty() || event.stream_id.len() > 128 || expect_snapshot != snapshot
        {
            return Err(ClientError::EventProtocol(
                "unexpected snapshot or stream identity",
            ));
        }
        if let Some((identity, cursor)) = &mut self.cursor
            && *identity == event.stream_id
        {
            if snapshot {
                if event.sequence == *cursor {
                    return Ok(None);
                }
                if event.sequence < *cursor {
                    return Err(ClientError::EventProtocol("snapshot cursor regressed"));
                }
            } else if cursor.checked_add(1) != Some(event.sequence) {
                return Err(ClientError::EventProtocol(
                    "live event sequence is not contiguous",
                ));
            }
            *cursor = event.sequence;
        } else if snapshot {
            self.cursor = Some((event.stream_id.clone(), event.sequence));
        } else {
            return Err(ClientError::EventProtocol(
                "stream identity changed before a snapshot",
            ));
        }
        Ok(Some(event))
    }
}

fn transient(status: &Status) -> bool {
    matches!(
        status.code(),
        Code::Unavailable | Code::Cancelled | Code::ResourceExhausted
    ) || peer_disconnected(status)
}
