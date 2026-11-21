//! Publish queue-ordered lifecycle snapshots and events without sharing framework objects.

use marsa_vm::{VmError, VmEvent, VmEventStream, VmExitCause, VmState};
use tokio::sync::{broadcast, watch};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

const EVENT_BUFFER: usize = 32;

pub(super) struct Lifecycle {
    state: watch::Sender<VmState>,
    events: broadcast::Sender<VmEvent>,
}

impl Lifecycle {
    pub(super) fn new() -> Self {
        let (state, _) = watch::channel(VmState::Created);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self { state, events }
    }

    pub(super) fn current(&self) -> VmState {
        *self.state.borrow()
    }

    pub(super) fn watch(&self) -> watch::Receiver<VmState> {
        self.state.subscribe()
    }

    pub(super) fn events(&self) -> VmEventStream {
        Box::pin(BroadcastStream::new(self.events.subscribe()).map(|event| {
            event.unwrap_or_else(
                |tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)| {
                    VmEvent::EventsDropped { count }
                },
            )
        }))
    }

    // Only the native serial queue publishes transitions.
    pub(super) fn transition(&self, current: VmState) {
        let previous = self.state.send_replace(current);
        if previous != current {
            let _ = self
                .events
                .send(VmEvent::StateChanged { previous, current });
        }
    }

    pub(super) fn unexpected_exit(&self, cause: VmExitCause, detail: String) {
        if !matches!(
            self.current(),
            VmState::Created | VmState::Stopped | VmState::Failed
        ) {
            self.transition(VmState::Failed);
            let _ = self.events.send(VmEvent::UnexpectedExit { cause, detail });
        }
    }
}

pub(super) fn backend(operation: &'static str, detail: impl Into<String>) -> VmError {
    VmError::Backend {
        operation,
        detail: detail.into(),
    }
}

pub(super) fn unsupported(capability: &'static str) -> VmError {
    VmError::Unsupported { capability }
}

pub(super) fn invalid_spec(field: &'static str, reason: impl Into<String>) -> VmError {
    VmError::InvalidSpec {
        field,
        reason: reason.into(),
    }
}
