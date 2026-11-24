//! Represent native host notifications as ordered daemon input.

#[cfg(target_os = "macos")]
pub mod platform;

use std::sync::mpsc::Sender;

use marsa_proto::host::v1::HostEventKind;
use tokio::sync::OwnedSemaphorePermit;

/// An observed change to host power or its default network path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    /// The host is about to enter system sleep.
    WillSleep,
    /// The host has finished waking.
    Woke,
    /// The host's default network path changed.
    NetworkChanged,
}

impl From<HostEvent> for HostEventKind {
    fn from(event: HostEvent) -> Self {
        match event {
            HostEvent::WillSleep => Self::WillSleep,
            HostEvent::Woke => Self::Woke,
            HostEvent::NetworkChanged => Self::NetworkChanged,
        }
    }
}

/// A host input with any native resources retained until it has been handled.
pub struct HostNotification {
    pub(crate) event: HostEvent,
    completed: Option<Sender<()>>,
    // A single pending path update suffices to tell consumers to refresh the path.
    _pending_path: Option<OwnedSemaphorePermit>,
}

impl From<HostEvent> for HostNotification {
    fn from(event: HostEvent) -> Self {
        Self {
            event,
            completed: None,
            _pending_path: None,
        }
    }
}

impl HostNotification {
    pub(crate) fn finish(self) {
        if let Some(completed) = self.completed {
            let _ = completed.send(());
        }
    }
}
