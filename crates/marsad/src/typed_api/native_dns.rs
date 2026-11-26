//! Snapshot the macOS resolver routing used for ordinary, unscoped DNS queries.

use std::io;

use marsa_core::settings::NetworkSettings;
use marsa_proto::guest::v1::{ConfigureNetwork, NativeDns};
use marsa_vm::NetworkSpec;

pub(super) fn configuration(
    network: &NetworkSettings,
    attachment: NetworkSpec,
    proxy: Option<ConfigureNetwork>,
) -> io::Result<Option<ConfigureNetwork>> {
    let mut policy = proxy.unwrap_or_default();
    policy.dns_servers = network.dns.iter().map(ToString::to_string).collect();
    if attachment == NetworkSpec::Nat {
        policy.native_dns = Some(NativeDns {
            explicit_servers: !network.dns.is_empty(),
            ..Default::default()
        });
        if network.dns.is_empty() {
            // A host with no usable resolver still starts its local Docker engine.
            // Send an empty route table on a failed snapshot: the guest then
            // serves aliases only, until the next network change.
            let result = platform::read(&mut policy).and_then(|()| {
                use marsa_proto::guest::{
                    encode_host,
                    v1::{HostEnvelope, host_envelope::Body},
                };
                encode_host(&HostEnvelope {
                    request_id: 1,
                    body: Some(Body::ConfigureNetwork(policy.clone())),
                })
                .map(|_| ())
                .map_err(io::Error::other)
            });
            if result.is_err() {
                tracing::warn!(
                    "host resolver snapshot unavailable; native DNS temporarily offline"
                );
                policy.dns_servers.clear();
                policy.search_domains.clear();
                policy.native_dns = Some(NativeDns::default());
            }
        }
    }
    Ok((policy != ConfigureNetwork::default()).then_some(policy))
}

impl super::TypedApi {
    pub(super) async fn refresh_native_dns(
        &self,
        supervisor: &crate::supervisor::Supervisor,
    ) -> io::Result<()> {
        let Some(session) = self
            .guest_control
            .as_ref()
            .and_then(|control| control.resolver_session())
        else {
            return Ok(());
        };
        if supervisor
            .snapshot()
            .network
            .is_none_or(|network| network.active != NetworkSpec::Nat)
        {
            return Ok(());
        }
        let network = self.inner.events.startup_settings().network;
        let Some(policy) =
            tokio::task::spawn_blocking(move || configuration(&network, NetworkSpec::Nat, None))
                .await??
        else {
            return Ok(());
        };
        session.configure(policy).await.map_err(Into::into)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::time::Duration;

    pub(super) fn read(policy: &mut ConfigureNetwork) -> io::Result<()> {
        // Both cold preparation and live refresh run on blocking workers.
        let bytes = tokio::runtime::Handle::current()
            .block_on(crate::platform_command::output(
                "/usr/sbin/scutil",
                &["--dns"],
                Duration::from_secs(2),
                128 * 1024,
            ))
            .ok_or(io::ErrorKind::InvalidData)?;
        super::parse(
            std::str::from_utf8(&bytes).map_err(io::Error::other)?,
            policy,
        )
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    pub(super) fn read(_: &mut ConfigureNetwork) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse(output: &str, policy: &mut ConfigureNetwork) -> io::Result<()> {
    use marsa_proto::guest::v1::DnsRoute;
    use std::{collections::BTreeMap, net::IpAddr};
    // The first section is what macOS uses for ordinary queries. Interface-scoped
    // and service-specific sections must not become global guest defaults.
    let ordinary = output.split("\nDNS configuration (").next().unwrap_or("");
    let mut routes = BTreeMap::new();
    let mut have_default = false;
    for section in ordinary.split("\nresolver #").skip(1) {
        let mut route = DnsRoute::default();
        let mut order = u32::MAX;
        let mut search = Vec::new();
        for line in section.lines().skip(1) {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "domain" => route.domain = value.trim_end_matches('.').to_ascii_lowercase(),
                "port" => route.port = value.parse::<u16>().map_err(io::Error::other)?.into(),
                "order" => order = value.parse().map_err(io::Error::other)?,
                key if key.starts_with("nameserver[") => {
                    // Host loopback and interface-local addresses have a different
                    // meaning in the VM. Keep such split domains local-only.
                    if let Ok(ip) = value.parse::<IpAddr>()
                        && !ip.is_loopback()
                        && !ip.is_unspecified()
                        && !matches!(ip, IpAddr::V6(ip) if ip.is_unicast_link_local())
                        && !matches!(ip, IpAddr::V4(ip) if ip.is_link_local())
                    {
                        route.servers.push(ip.to_string());
                    }
                }
                key if key.starts_with("search domain[") => search.push(value.to_owned()),
                _ => (),
            }
        }
        if route.domain.is_empty() {
            if !have_default {
                // The default resolver format carries standard port 53.
                // A host-only custom port must not fall back to public DNS.
                if matches!(route.port, 0 | 53) {
                    policy.dns_servers = route.servers;
                }
                policy.search_domains = search;
                have_default = true;
            }
        } else {
            let entry = routes
                .entry(route.domain.clone())
                .or_insert((order, route.clone()));
            if order < entry.0 {
                *entry = (order, route);
            }
        }
    }
    policy.native_dns = Some(NativeDns {
        explicit_servers: false,
        routes: routes.into_values().map(|(_, route)| route).collect(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_only_resolvers_keep_default_and_private_domains_closed() {
        let snapshot = "DNS configuration\n\nresolver #1\n nameserver[0] : 127.0.0.1\n\nresolver #2\n domain : private.test\n nameserver[0] : fe80::1%en0\n";
        let mut policy = ConfigureNetwork::default();
        parse(snapshot, &mut policy).unwrap();
        assert!(policy.dns_servers.is_empty());
        assert!(policy.native_dns.unwrap().routes[0].servers.is_empty());
    }

    #[test]
    fn separates_default_search_and_supplemental_routes_after_handoff() {
        let snapshot = "DNS configuration\n\nresolver #1\n search domain[0] : corp.test\n nameserver[0] : 192.0.2.53\n\nresolver #2\n domain : corp.test\n nameserver[0] : 10.0.0.53\n port : 5353\n order : 10\n\nresolver #3\n domain : local\n options : mdns\n\nDNS configuration (for scoped queries)\n\nresolver #1\n nameserver[0] : 198.51.100.99\n";
        let mut policy = ConfigureNetwork::default();
        parse(snapshot, &mut policy).unwrap();
        assert_eq!(policy.dns_servers, ["192.0.2.53"]);
        assert_eq!(policy.search_domains, ["corp.test"]);
        let routes = policy.native_dns.unwrap().routes;
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].servers, ["10.0.0.53"]);
        assert_eq!(routes[0].port, 5353);
        assert!(routes[1].servers.is_empty());
    }
}
