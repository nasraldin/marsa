//! Publish daemon observations only while their exact session is active.

use std::sync::{Arc, Weak};

use tokio::sync::{broadcast, watch};

use super::{EVENT_CAPACITY, GuestEvent, session::Client};

type Observer = Arc<dyn Fn(&GuestEvent) + Send + Sync>;

#[derive(Clone)]
pub(super) struct EventBus {
    events: broadcast::Sender<GuestEvent>,
    pub(super) observer: Option<Observer>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            events: broadcast::channel(EVENT_CAPACITY).0,
            observer: None,
        }
    }
}

impl EventBus {
    pub(super) fn subscribe(&self) -> broadcast::Receiver<GuestEvent> {
        self.events.subscribe()
    }

    pub(super) fn session(
        self,
        active: watch::Sender<Weak<Client>>,
        source: Weak<Client>,
    ) -> SessionEvents {
        SessionEvents {
            bus: self,
            active,
            source,
        }
    }
}

#[derive(Clone)]
pub(super) struct SessionEvents {
    bus: EventBus,
    active: watch::Sender<Weak<Client>>,
    source: Weak<Client>,
}

impl SessionEvents {
    pub(super) fn publish(&self, event: GuestEvent) {
        self.publish_active(&event);
        let _ = self.bus.events.send(event);
    }

    pub(super) fn publish_active(&self, event: &GuestEvent) {
        let Some(observer) = &self.bus.observer else {
            return;
        };
        // Hold the read guard until publication. Revocation cannot interleave
        // with the write to the daemon's snapshot.
        let active = self.active.borrow();
        if active.ptr_eq(&self.source) {
            observer(event);
        }
    }
}
