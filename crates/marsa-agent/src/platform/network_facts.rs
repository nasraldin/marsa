//! Report upstream interface addresses and routes from the guest kernel.

use std::{io, net::Ipv4Addr, time::Duration};

use marsa_core::guest_boot::Settings;
use marsa_proto::guest::{NetworkFacts, encode_network_facts};
use nix::ifaddrs::getifaddrs;
use tokio::{io::AsyncWriteExt, task::spawn_blocking, time::timeout};

use super::{operations::stats, vsock::Listener};

pub(super) fn interface() -> io::Result<Option<String>> {
    let cmdline = stats::read_bounded("/proc/cmdline", 4096)?;
    Ok(Settings::parse(&cmdline)?.interface)
}

pub(super) async fn serve(listener: Listener, interface: Option<String>) {
    while let Ok(mut stream) = listener.accept_host().await {
        let interface = interface.clone();
        let sample = spawn_blocking(move || interface.and_then(|name| facts(&name).ok().flatten()));
        let facts = sample.await.ok().flatten();
        let _ = timeout(
            Duration::from_secs(1),
            stream.write_all(&encode_network_facts(facts)),
        )
        .await;
    }
}

pub(super) fn facts(interface: &str) -> io::Result<Option<NetworkFacts>> {
    let address = getifaddrs()?
        .filter(|item| item.interface_name == interface)
        .find_map(|item| Some(item.address?.as_sockaddr_in()?.ip()))
        .filter(|address| !address.is_unspecified() && !address.is_loopback());
    let Some(address) = address else {
        return Ok(None);
    };
    let routes = stats::read_bounded("/proc/net/route", 64 * 1024)?;
    Ok(Some(NetworkFacts {
        address,
        gateway: default_gateway(&routes, interface),
    }))
}

fn default_gateway(routes: &str, interface: &str) -> Option<Ipv4Addr> {
    routes
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            if fields.next()? != interface || fields.next()? != "00000000" {
                return None;
            }
            let gateway = u32::from_str_radix(fields.next()?, 16).ok()?;
            let flags = u32::from_str_radix(fields.next()?, 16).ok()?;
            let metric = fields.nth(2)?.parse::<u32>().ok()?;
            // RTF_UP | RTF_GATEWAY, with a zero destination and netmask.
            (flags & 3 == 3 && gateway != 0 && fields.next()? == "00000000")
                .then_some((metric, Ipv4Addr::from(gateway.to_ne_bytes())))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, gateway)| gateway)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lowest_metric_default_route_for_the_upstream_device() {
        let routes = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
            docker0 00000000 010011AC 0003 0 0 0 00000000 0 0 0\n\
            eth0 00000000 0164A8C0 0003 0 0 42 00000000 0 0 0\n\
            eth0 00000000 0140A8C0 0003 0 0 1 00000000 0 0 0\n\
            eth0 00000000 0150A8C0 0002 0 0 0 00000000 0 0 0\n";
        assert_eq!(
            default_gateway(routes, "eth0"),
            Some(Ipv4Addr::new(192, 168, 64, 1))
        );
        assert_eq!(default_gateway(routes, "eth1"), None);
    }
}
