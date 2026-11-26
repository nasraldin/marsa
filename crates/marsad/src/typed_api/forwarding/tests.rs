//! Check policy changes and recovery using real binds with an inert guest connector.

use super::*;

// These tests intentionally close and reopen the same listener. Port zero can
// assign an ephemeral port subsequently consumed by an unrelated outgoing socket.
fn reserve_test_port() -> std::net::TcpListener {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    for _ in 0..4096 {
        let port = 24000 + NEXT.fetch_add(1, Ordering::Relaxed) % 4096;
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(socket) => return socket,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(error) => panic!("could not bind test port: {error}"),
        }
    }
    panic!("no free port for the rebind test")
}

fn event(ip: &str, port: u16) -> PortEvent {
    PortEvent {
        container_id: "a".repeat(64),
        bind_address: ip.to_owned(),
        published_port: u32::from(port),
        container_port: 80,
        transport: GuestTransport::Tcp.into(),
        action: PortAction::Opened.into(),
    }
}

#[test]
fn over_budget_inventory_revokes_every_listener_until_session_resync() {
    let ports = Ports::new(false);
    for p in 1..=crate::guest_control::MAX_PORT_BINDINGS + 1 {
        ports.event(&event("0.0.0.0", p as u16));
    }
    assert!(ports.desired.borrow().saturated);
    assert!(ports.desired.borrow().bindings.is_empty());
    ports.event(&event("0.0.0.0", 49000));
    assert!(ports.desired.borrow().bindings.is_empty());
    ports.clear();
    ports.event(&event("0.0.0.0", 49000));
    assert_eq!(ports.desired.borrow().bindings.len(), 1);
}

#[tokio::test]
async fn wildcard_policy_rebinds_without_widening_explicit_addresses() {
    let ports = Ports::new(false);
    let occupied = reserve_test_port();
    let number = occupied.local_addr().unwrap().port();
    let requested = event("0.0.0.0", number);
    let connect: Connector =
        Arc::new(|| Box::pin(async { Err(io::ErrorKind::NotConnected.into()) }));
    let permits = Arc::new(Semaphore::new(1));
    let mut live = BTreeMap::new();
    ports.event(&requested);
    let desired = ports.desired.borrow().clone();
    let mut report = reconcile(&desired, &mut live, &connect, &permits, MAX_LISTENERS).await;
    assert!(report.ports.is_empty());
    assert_eq!(report.failures[0].host_address, "127.0.0.1");
    assert!(report.failures[0].message.contains(&number.to_string()));
    owner::identify(&mut report.failures).await;
    #[cfg(target_os = "macos")]
    assert!(
        report.failures[0]
            .owner
            .as_ref()
            .is_some_and(|owner| owner.contains(&std::process::id().to_string()))
    );
    drop(occupied);
    assert_eq!(
        reconcile(&desired, &mut live, &connect, &permits, MAX_LISTENERS)
            .await
            .ports[0]
            .host_address,
        "127.0.0.1"
    );
    ports.policy(true).await;
    let desired = ports.desired.borrow().clone();
    assert_eq!(
        reconcile(&desired, &mut live, &connect, &permits, MAX_LISTENERS)
            .await
            .ports[0]
            .host_address,
        "0.0.0.0"
    );
    ports.policy(false).await;
    let desired = ports.desired.borrow().clone();
    assert_eq!(
        reconcile(&desired, &mut live, &connect, &permits, MAX_LISTENERS)
            .await
            .ports[0]
            .host_address,
        "127.0.0.1"
    );
    ports.clear();
    let desired = ports.desired.borrow().clone();
    assert!(
        reconcile(&desired, &mut live, &connect, &permits, MAX_LISTENERS)
            .await
            .ports
            .is_empty()
    );
    assert!(live.is_empty());
    let local = Binding::from_event(&event("127.0.0.1", number)).unwrap();
    assert_eq!(local.host_address(true).unwrap().ip(), Ipv4Addr::LOCALHOST);
    let specific = Binding::from_event(&event("192.0.2.1", number)).unwrap();
    assert!(specific.host_address(true).is_err());
    assert!(specific.host_address(false).is_err());
}

#[tokio::test]
async fn freed_listener_capacity_restores_a_previously_reported_request() {
    let ports = Ports::new(false);
    let a = reserve_test_port();
    let b = reserve_test_port();
    let first = event("127.0.0.1", a.local_addr().unwrap().port());
    let second = event("127.0.0.1", b.local_addr().unwrap().port());
    drop((a, b));
    ports.event(&first);
    ports.event(&second);
    let connect: Connector =
        Arc::new(|| Box::pin(async { Err(io::ErrorKind::NotConnected.into()) }));
    let permits = Arc::new(Semaphore::new(1));
    let mut live = BTreeMap::new();
    let desired = ports.desired.borrow().clone();
    let report = reconcile(&desired, &mut live, &connect, &permits, 1).await;
    assert_eq!(report.ports.len(), 1);
    assert_eq!(report.failures.len(), 1);
    assert!(report.failures[0].message.contains("limit reached"));
    let mut closed = if report.ports[0].host_port == first.published_port {
        first
    } else {
        second
    };
    closed.action = PortAction::Closed.into();
    ports.event(&closed);
    let desired = ports.desired.borrow().clone();
    let restored = reconcile(&desired, &mut live, &connect, &permits, 1).await;
    assert_eq!(restored.ports.len(), 1);
    assert_eq!(restored.ports[0].host_port, report.failures[0].host_port);
    assert!(restored.failures.is_empty());
}

#[tokio::test]
async fn disabling_lan_waits_for_wildcard_connections_to_close() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let ports = Ports::new(true);
    let bus = events::Bus::new(Default::default()).unwrap();
    let connect: Connector = Arc::new(|| {
        Box::pin(async {
            let (host, mut guest) = tokio::io::duplex(64);
            tokio::spawn(async move {
                let mut header = [0; 5];
                guest.read_exact(&mut header).await.unwrap();
                guest.write_u8(0).await.unwrap();
                let (mut reader, mut writer) = tokio::io::split(guest);
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
            Ok(Box::new(host) as Box<dyn marsa_net::RelayStream>)
        })
    });
    ports.serve(connect, bus.clone());
    let address = reserve_test_port().local_addr().unwrap();
    ports.event(&event("0.0.0.0", address.port()));
    ports.policy(true).await;
    assert_eq!(bus.forwarded_ports().ports[0].host_address, "0.0.0.0");
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client.write_u8(42).await.unwrap();
    assert_eq!(client.read_u8().await.unwrap(), 42);
    ports.policy(false).await;
    assert_eq!(bus.forwarded_ports().ports[0].host_address, "127.0.0.1");
    assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
}
