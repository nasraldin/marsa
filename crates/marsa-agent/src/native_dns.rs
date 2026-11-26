//! Render dnsmasq upstream routes from validated guest control messages.

use marsa_proto::guest::v1::ConfigureNetwork;
use std::{fmt::Write as _, io, net::IpAddr};

pub(crate) fn servers(defaults: &str, request: &ConfigureNetwork) -> io::Result<String> {
    let mut output = String::new();
    if request.dns_servers.is_empty() && request.native_dns.is_none() {
        for line in defaults.lines() {
            let mut fields = line.split_ascii_whitespace();
            if fields.next() == Some("nameserver")
                && let Some(address) = fields.next().and_then(|s| s.parse::<IpAddr>().ok())
            {
                writeln!(output, "server={address}").map_err(io::Error::other)?;
            }
        }
    } else {
        for server in &request.dns_servers {
            let address = server.parse::<IpAddr>().map_err(io::Error::other)?;
            writeln!(output, "server={address}").map_err(io::Error::other)?;
        }
    }
    if let Some(native) = &request.native_dns {
        for route in &native.routes {
            let domain = route.domain.trim_end_matches('.');
            if route.servers.is_empty() {
                writeln!(output, "server=/{domain}/").map_err(io::Error::other)?;
            }
            for server in &route.servers {
                let address = server.parse::<IpAddr>().map_err(io::Error::other)?;
                let port = if route.port == 0 { 53 } else { route.port };
                writeln!(output, "server=/{domain}/{address}#{port}").map_err(io::Error::other)?;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsa_proto::guest::v1::{DnsRoute, NativeDns};

    #[test]
    fn native_policy_replaces_dhcp_and_keeps_private_queries_scoped() {
        let dhcp = "nameserver 192.0.2.1\n";
        let mut request = ConfigureNetwork::default();
        assert_eq!(servers(dhcp, &request).unwrap(), "server=192.0.2.1\n");
        request.native_dns = Some(NativeDns {
            explicit_servers: false,
            routes: vec![
                DnsRoute {
                    domain: "corp.test".to_owned(),
                    servers: vec!["10.1.1.53".into()],
                    port: 5353,
                },
                DnsRoute {
                    domain: "private.test".to_owned(),
                    ..Default::default()
                },
            ],
        });
        assert_eq!(
            servers(dhcp, &request).unwrap(),
            "server=/corp.test/10.1.1.53#5353\nserver=/private.test/\n"
        );
        request.dns_servers = vec!["198.51.100.53".into()];
        assert_eq!(
            servers(dhcp, &request).unwrap(),
            "server=198.51.100.53\nserver=/corp.test/10.1.1.53#5353\nserver=/private.test/\n"
        );
    }
}
