//! Keep state transitions and their notifications ordered across fake VM tasks.

use std::sync::Mutex;

use tokio::sync::broadcast;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

use crate::{VmError, VmEvent, VmEventStream, VmExitCause, VmState};

use super::lock;

const EVENT_BUFFER: usize = 32;

pub(super) struct Lifecycle {
    state: Mutex<VmState>,
    events: broadcast::Sender<VmEvent>,
}

impl Lifecycle {
    pub(super) fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self {
            state: Mutex::new(VmState::Created),
            events,
        }
    }

    pub(super) fn state(&self) -> VmState {
        *lock(&self.state)
    }

    pub(super) fn events(&self) -> VmEventStream {
        Box::pin(
            BroadcastStream::new(self.events.subscribe()).map(|result| match result {
                Ok(event) => event,
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
                    VmEvent::EventsDropped { count }
                }
            }),
        )
    }

    pub(super) fn network_unavailable(&self) {
        let _ = self.events.send(VmEvent::NetworkUnavailable {
            detail: "scripted network transport failure".into(),
        });
    }

    pub(super) fn exit_unexpectedly(&self) -> bool {
        let event = VmEvent::UnexpectedExit {
            cause: VmExitCause::Failure,
            detail: "scripted guest exit".into(),
        };
        self.transition_with(
            "exit",
            &[VmState::Running, VmState::Paused],
            VmState::Failed,
            Some(event),
        )
        .is_ok()
    }

    pub(super) fn transition(
        &self,
        operation: &'static str,
        allowed: &[VmState],
        current: VmState,
    ) -> Result<(), VmError> {
        self.transition_with(operation, allowed, current, None)
    }

    fn transition_with(
        &self,
        operation: &'static str,
        allowed: &[VmState],
        current: VmState,
        event: Option<VmEvent>,
    ) -> Result<(), VmError> {
        let mut state = lock(&self.state);
        let previous = *state;
        if !allowed.contains(&previous) {
            return Err(VmError::InvalidState {
                operation,
                state: previous,
            });
        }
        *state = current;
        let _ = self
            .events
            .send(VmEvent::StateChanged { previous, current });
        if let Some(event) = event {
            let _ = self.events.send(event);
        }
        Ok(())
    }

    pub(super) fn running(&self, operation: &'static str) -> Result<(), VmError> {
        let state = *lock(&self.state);
        if state == VmState::Running {
            Ok(())
        } else {
            Err(VmError::InvalidState { operation, state })
        }
    }
}
