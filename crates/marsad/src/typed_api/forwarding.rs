//! Reconcile the active guest's publications with the owner's current exposure policy.

use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use marsa_net::{Connector, Listener, Transport};
use marsa_proto::{
    guest::v1::{PortAction, PortEvent, Transport as GuestTransport},
    host::v1::{ForwardedPort, ForwardedPortFailure, ListForwardedPortsResponse, PortTransport},
};
use tokio::sync::{Semaphore, watch};

use super::events;

mod owner;

// Completed and failed requests retain only their small inventory rows. The smaller
// live budget also bounds kernel file descriptors and per-listener datagram buffers.
const MAX_LISTENERS: usize = 256;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Binding {
    container: String,
    published: SocketAddr,
    guest_port: u16,
    protocol: Transport,
}

impl Binding {
    fn from_event(event: &PortEvent) -> Option<Self> {
        Some(Self {
            container: event.container_id.clone(),
            published: SocketAddr::new(
                event.bind_address.parse().ok()?,
                event.published_port.try_into().ok()?,
            ),
            guest_port: event.container_port.try_into().ok()?,
            protocol: match event.transport() {
                GuestTransport::Tcp => Transport::Tcp,
                GuestTransport::Udp => Transport::Udp,
                GuestTransport::Unspecified => return None,
            },
        })
    }

    fn host_address(&self, lan: bool) -> io::Result<SocketAddr> {
        let ip = self.published.ip();
        let local = match ip {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        };
        if !ip.is_unspecified() && ip != local {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "publish on wildcard or localhost to use the guest loopback relay | استخدم عنوان الاستماع العام أو المضيف المحلي لتحويل المنفذ",
            ));
        }
        Ok(if lan {
            self.published
        } else {
            SocketAddr::new(local, self.published.port())
        })
    }

    fn wire_transport(&self) -> i32 {
        match self.protocol {
            Transport::Tcp => PortTransport::Tcp.into(),
            Transport::Udp => PortTransport::Udp.into(),
        }
    }

    fn row(&self, host: SocketAddr) -> ForwardedPort {
        ForwardedPort {
            host_address: host.ip().to_string(),
            host_port: u32::from(host.port()),
            guest_port: u32::from(self.guest_port),
            transport: self.wire_transport(),
            container_id: self.container.clone(),
        }
    }

    fn failure(&self, host: SocketAddr, error: io::Error) -> ForwardedPortFailure {
        let message = match error.kind() {
            io::ErrorKind::AddrInUse => {
                format!("{host} is already in use | العنوان {host} قيد الاستخدام")
            }
            io::ErrorKind::PermissionDenied if host.port() < 1024 => format!(
                "Permission denied for {host}. Choose a host port of 1024 or higher. | رُفض الإذن للعنوان {host}. اختر منفذ مضيف رقمه 1024 أو أكبر."
            ),
            _ => format!("{host}: {error}"),
        };
        ForwardedPortFailure {
            host_address: host.ip().to_string(),
            host_port: u32::from(host.port()),
            transport: self.wire_transport(),
            container_id: self.container.clone(),
            message,
            owner: None,
        }
    }
}

#[derive(Clone, Default)]
struct Desired {
    revision: u64,
    lan: bool,
    // A close and reopen between reconciliations must still terminate old clients.
    bindings: BTreeMap<Binding, u64>,
    saturated: bool,
}

pub(super) struct Ports {
    desired: watch::Sender<Desired>,
    applied: watch::Sender<u64>,
}

impl Ports {
    pub(super) fn new(lan: bool) -> Self {
        Self {
            desired: watch::channel(Desired {
                lan,
                ..Desired::default()
            })
            .0,
            applied: watch::channel(0).0,
        }
    }

    pub(super) fn event(&self, event: &PortEvent) {
        let Some(binding) = Binding::from_event(event) else {
            return;
        };
        self.desired.send_if_modified(|state| {
            if state.saturated {
                return false;
            }
            let changed = match event.action() {
                PortAction::Opened => {
                    if state.bindings.contains_key(&binding) {
                        return false;
                    }
                    // GuestControl disconnects on overflow; fail closed even if called
                    // by another event producer. Only a fresh session can restore grants.
                    if state.bindings.len() >= crate::guest_control::MAX_PORT_BINDINGS {
                        state.bindings.clear();
                        state.saturated = true;
                        tracing::warn!("published port capacity exceeded; revoked host listeners");
                    } else {
                        state.bindings.insert(binding, state.revision + 1);
                    }
                    true
                }
                PortAction::Closed => state.bindings.remove(&binding).is_some(),
                PortAction::Unspecified => false,
            };
            if changed {
                state.revision += 1;
            }
            changed
        });
    }

    pub(super) fn clear(&self) {
        self.desired.send_if_modified(|state| {
            if state.bindings.is_empty() && !state.saturated {
                return false;
            }
            state.saturated = false;
            state.bindings.clear();
            state.revision += 1;
            true
        });
    }

    pub(super) async fn policy(&self, lan: bool) {
        let mut applied = self.applied.subscribe();
        self.desired.send_if_modified(|state| {
            if state.lan == lan {
                return false;
            }
            state.lan = lan;
            state.revision += 1;
            true
        });
        let revision = self.desired.borrow().revision;
        if self.desired.receiver_count() != 0 {
            let _ = applied.wait_for(|seen| *seen >= revision).await;
        }
    }

    pub(super) fn serve(&self, connect: Connector, bus: events::Bus) {
        let mut desired = self.desired.subscribe();
        let applied = self.applied.clone();
        tokio::spawn(async move {
            let mut live = BTreeMap::new();
            let permits = Arc::new(Semaphore::new(256));
            loop {
                let current = desired.borrow_and_update().clone();
                let mut report =
                    reconcile(&current, &mut live, &connect, &permits, MAX_LISTENERS).await;
                bus.ports(report.clone());
                applied.send_replace(current.revision);
                if report.failures.is_empty() {
                    if desired.changed().await.is_err() {
                        break;
                    }
                } else {
                    tokio::select! {
                        changed = desired.changed() => if changed.is_err() { break; },
                        () = async {
                            owner::identify(&mut report.failures).await;
                            bus.ports(report);
                            tokio::time::sleep(Duration::from_secs(3)).await;
                        } => (),
                    }
                }
            }
            for (_, running) in live {
                running.listener.shutdown().await;
            }
        });
    }
}

struct Running {
    host: SocketAddr,
    generation: u64,
    listener: Listener,
}

async fn reconcile(
    current: &Desired,
    live: &mut BTreeMap<Binding, Running>,
    connect: &Connector,
    permits: &Arc<Semaphore>,
    limit: usize,
) -> ListForwardedPortsResponse {
    let stale: Vec<_> = live
        .iter()
        .filter(|(key, running)| {
            current.bindings.get(*key) != Some(&running.generation)
                || key.host_address(current.lan).ok() != Some(running.host)
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in stale {
        if let Some(running) = live.remove(&key) {
            running.listener.shutdown().await;
        }
    }
    let mut report = ListForwardedPortsResponse::default();
    for (key, generation) in &current.bindings {
        let host = match key.host_address(current.lan) {
            Ok(host) => host,
            Err(error) => {
                report.failures.push(key.failure(key.published, error));
                continue;
            }
        };
        if !live.contains_key(key) {
            if live.len() >= limit {
                let reason = io::Error::other(
                    "host listener limit reached; close another publication to retry | تم بلوغ حد منافذ المضيف. أغلق منفذاً منشوراً آخر لإعادة المحاولة",
                );
                report.failures.push(key.failure(host, reason));
                continue;
            }
            match Listener::bind(host, key.protocol, connect.clone(), permits.clone()) {
                Ok(listener) => {
                    live.insert(
                        key.clone(),
                        Running {
                            host,
                            generation: *generation,
                            listener,
                        },
                    );
                }
                Err(error) => {
                    report.failures.push(key.failure(host, error));
                    continue;
                }
            }
        }
        report.ports.push(key.row(host));
    }
    report
}

#[cfg(test)]
mod tests;
