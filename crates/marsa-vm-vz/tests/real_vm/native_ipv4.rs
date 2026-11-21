//! Qualify direct IPv4 HTTPS through Apple's native NAT attachment.

use std::{
    env,
    net::{SocketAddr, ToSocketAddrs as _},
    path::Path,
    time::Duration,
};

use marsa_proto::guest::v1::EgressStage;
use marsa_vm::Hypervisor;
use marsa_vm_vz::VzHypervisor;
use tokio::time::timeout;

use super::{
    GuestBody, HostBody, LIMIT, connect_control, console_with, negotiate_and_set_time,
    network_with_gateway, read_guest, send_host, spec,
};

const CAPTURE_NETWORK: &str = concat!(
    "console=hvc0 root=/dev/vda rootfstype=erofs ro panic=-1 init=/bin/sh -- -c \"",
    "exec </dev/hvc0 >/dev/hvc0 2>&1; (",
    "while ! /sbin/ip -4 route show default dev eth0 | /bin/busybox grep -q '^default via '; do ",
    "/bin/busybox sleep 1; done; ",
    "/sbin/ip -4 -o addr show dev eth0; ",
    "/sbin/ip -4 route show default dev eth0; echo MARSA_NET_CONFIG_CAPTURED; ",
    ") & exec /usr/bin/marsa-init\"",
);

#[tokio::test]
#[ignore = "requires virtualization entitlement, native NAT and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_guest_nat_address_gateway_and_https() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let spec = spec(&guest, scratch.path(), CAPTURE_NETWORK);
    let console = spec.console_log.clone();
    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(vm.network_info().is_none());
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();

    let mut stream = connect_control(vm.as_ref(), &console).await;
    timeout(LIMIT, negotiate_and_set_time(&mut stream, true))
        .await
        .unwrap();
    let network = network_with_gateway(vm.as_ref(), &console).await;
    let log = console_with(&console, "MARSA_NET_CONFIG_CAPTURED\r\n").await;
    assert!(network.address.is_ipv4());
    assert!(
        log.lines()
            .any(|line| line.contains(&format!("eth0    inet {}/", network.address))),
        "{log}"
    );
    let gateway = network.gateway.expect("native NAT default gateway");
    assert!(gateway.is_ipv4());
    let gateway_words = ["default", "via", &gateway.to_string()];
    assert!(
        log.lines()
            .any(|line| line.split_ascii_whitespace().take(3).eq(gateway_words)),
        "{log}"
    );
    println!(
        "guest DHCP address {} via {gateway} matches the backend snapshot",
        network.address
    );

    let destination = timeout(
        LIMIT,
        tokio::task::spawn_blocking(|| {
            ("registry-1.docker.io", 443)
                .to_socket_addrs()
                .expect("resolve the Docker registry on the host")
                .find_map(|address| match address {
                    SocketAddr::V4(address) => Some(*address.ip()),
                    SocketAddr::V6(_) => None,
                })
                .expect("Docker registry has an IPv4 address")
        }),
    )
    .await
    .unwrap()
    .unwrap();
    // The guest probe verifies the IPv4 DNS answer, uses a SocketAddrV4, checks the
    // registry's TLS certificate, and requires a genuine HTTP registry response.
    let stage = timeout(LIMIT, async {
        send_host(
            &mut stream,
            2,
            HostBody::ProbeEgress(marsa_proto::guest::v1::ProbeEgress {
                address_v4: destination.octets().to_vec(),
            }),
        )
        .await;
        loop {
            let response = read_guest(&mut stream).await;
            if response.request_id == 2 {
                let Some(GuestBody::EgressResult(result)) = response.body else {
                    panic!("expected an egress result, got {response:?}");
                };
                break result.stage();
            }
        }
    })
    .await
    .unwrap();

    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    assert!(vm.network_info().is_none());
    assert_eq!(
        stage,
        EgressStage::Healthy,
        "native NAT IPv4 HTTPS to {destination} failed at {stage:?}"
    );
}
