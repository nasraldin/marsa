//! Publish daemon observations only while their exact session is active.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use marsa_proto::guest::v1::{PortAction, PortEvent};

use tokio::sync::{broadcast, watch};

use super::{EVENT_CAPACITY, GuestControlError, GuestEvent, session::Client};

type PortKey = (String, String, u32, u32, i32);

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
            ports: Arc::default(),
            trust_staged: Arc::default(),
        }
    }
}

#[derive(Clone)]
pub(super) struct SessionEvents {
    bus: EventBus,
    active: watch::Sender<Weak<Client>>,
    source: Weak<Client>,
    ports: Arc<Mutex<BTreeMap<PortKey, PortEvent>>>,
    trust_staged: Arc<AtomicBool>,
}

impl SessionEvents {
    pub(super) fn trust_staged(&self) {
        self.trust_staged.store(true, Ordering::Release);
        self.publish_active(&GuestEvent::TrustRestartRequired);
    }

    pub(super) fn publish(&self, event: GuestEvent) -> Result<(), GuestControlError> {
        if let GuestEvent::Port(port) = &event {
            // The guest starts its inventory scan before the supervisor verifies readiness.
            // Serialize that scan with activation so existing containers are replayed exactly once.
            let mut ports = self.ports.lock().unwrap_or_else(|error| error.into_inner());
            let key = (
                port.container_id.clone(),
                port.bind_address.clone(),
                port.published_port,
                port.container_port,
                port.transport,
            );
            if port.action() == PortAction::Opened {
                if ports.len() >= super::MAX_PORT_BINDINGS && !ports.contains_key(&key) {
                    return Err(GuestControlError::Sequence(
                        "published port inventory exceeds host limit",
                    ));
                }
                ports.insert(key, port.clone());
            } else {
                ports.remove(&key);
            }
            self.publish_active(&event);
        } else {
            self.publish_active(&event);
        }
        let _ = self.bus.events.send(event);
        Ok(())
    }

    pub(super) fn activate(&self) {
        let ports = self.ports.lock().unwrap_or_else(|error| error.into_inner());
        self.active.send_modify(|active| {
            *active = self.source.clone();
            if let Some(observer) = &self.bus.observer {
                observer(&GuestEvent::PortsCleared);
                if self.trust_staged.load(Ordering::Acquire) {
                    observer(&GuestEvent::TrustRestartRequired);
                }
                for port in ports.values() {
                    observer(&GuestEvent::Port(port.clone()));
                }
            }
        });
    }

    pub(super) fn revoke(&self) {
        self.active.send_if_modified(|active| {
            if !active.ptr_eq(&self.source) {
                return false;
            }
            *active = Weak::new();
            if let Some(observer) = &self.bus.observer {
                observer(&GuestEvent::PortsCleared);
            }
            true
        });
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
