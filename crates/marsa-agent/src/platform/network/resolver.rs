//! Render resolver data and dockerd's supported startup configuration.

use std::{collections::BTreeSet, fmt::Write as _, io, net::IpAddr};

use marsa_proto::guest::v1::{ConfigureNetwork, Proxy};
use serde_json::{Map, Value, json};

use super::invalid;

pub(super) fn resolv_conf(defaults: &str, request: &ConfigureNetwork) -> io::Result<String> {
    if request.dns_servers.is_empty() && request.search_domains.is_empty() {
        return Ok(defaults.to_owned());
    }
    let mut output = "# Written by marsa-agent\n".to_owned();
    for server in &request.dns_servers {
        let address = server
            .parse::<IpAddr>()
            .map_err(|_| invalid("invalid DNS address"))?;
        writeln!(output, "nameserver {address}").map_err(io::Error::other)?;
    }
    if request.dns_servers.is_empty() {
        for line in defaults
            .lines()
            .filter(|line| line.trim_start().starts_with("nameserver "))
        {
            writeln!(output, "{line}").map_err(io::Error::other)?;
        }
    }
    if !request.search_domains.is_empty() {
        let domains = request
            .search_domains
            .iter()
            .map(|domain| valid_name(domain))
            .collect::<io::Result<Vec<_>>>()?;
        writeln!(output, "search {}", domains.join(" ")).map_err(io::Error::other)?;
    }
    Ok(output)
}

pub(super) fn hosts_file(defaults: &str, request: &ConfigureNetwork) -> io::Result<String> {
    let mut output = defaults.to_owned();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    let aliases = request
        .host_aliases
        .iter()
        .map(|alias| {
            let address = alias
                .address
                .parse::<IpAddr>()
                .map_err(|_| invalid("invalid host alias address"))?;
            Ok((address, valid_name(&alias.name)?))
        })
        .collect::<io::Result<BTreeSet<_>>>()?;
    for (address, name) in aliases {
        writeln!(output, "{address}\t{name}").map_err(io::Error::other)?;
    }
    Ok(output)
}

pub(super) fn daemon_config(
    original: &[u8],
    request: &ConfigureNetwork,
) -> io::Result<Option<Vec<u8>>> {
    let current: Map<String, Value> = serde_json::from_slice(original)
        .map_err(|_| invalid("invalid guest Docker configuration"))?;
    let mut updated = current.clone();
    if let Some(registries) = &request.registries {
        for (key, values) in [
            ("registry-mirrors", &registries.mirrors),
            ("insecure-registries", &registries.insecure),
        ] {
            if values.is_empty() {
                updated.remove(key);
            } else {
                updated.insert(key.into(), json!(values));
            }
        }
    }
    if let Some(proxy) = request.proxy.as_ref().map(docker_proxy).transpose()?
        && !proxy.is_empty()
    {
        updated.insert("proxies".to_owned(), Value::Object(proxy));
    } else {
        updated.remove("proxies");
    }
    let gateway = request
        .host_aliases
        .iter()
        .filter(|alias| alias.name == "host.docker.internal")
        .map(|alias| {
            alias
                .address
                .parse::<IpAddr>()
                .map_err(|_| invalid("invalid host gateway IP address"))
        })
        .collect::<io::Result<BTreeSet<_>>>()?;
    if gateway.is_empty() {
        if request.native_dns.is_none() {
            updated.remove("host-gateway-ips");
        }
    } else {
        let ipv4 = gateway.iter().filter(|address| address.is_ipv4()).count();
        let ipv6 = gateway.len() - ipv4;
        if ipv4 > 1 || ipv6 > 1 {
            return Err(invalid(
                "host gateway allows at most one address per family",
            ));
        }
        let addresses = gateway
            .into_iter()
            .map(|address| address.to_string())
            .collect::<Vec<_>>();
        updated.insert("host-gateway-ips".to_owned(), json!(addresses));
    }
    if current == updated {
        return Ok(None);
    }
    let mut bytes = serde_json::to_vec_pretty(&updated).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(Some(bytes))
}

fn docker_proxy(proxy: &Proxy) -> io::Result<Map<String, Value>> {
    let mut result = Map::new();
    for (name, url) in [
        ("http-proxy", &proxy.http_url),
        ("https-proxy", &proxy.https_url),
    ] {
        if let Some(url) = url {
            let parsed: http::Uri = url.parse().map_err(|_| invalid("invalid proxy endpoint"))?;
            if !matches!(parsed.scheme_str(), Some("http" | "https"))
                || parsed.host().is_none()
                || parsed.query().is_some()
                || !matches!(parsed.path(), "" | "/")
            {
                return Err(invalid("invalid proxy endpoint"));
            }
            result.insert(name.to_owned(), json!(url));
        }
    }
    if !proxy.no_proxy.is_empty() {
        if proxy.no_proxy.iter().any(|host| {
            host.chars().any(|character| {
                character.is_control() || character.is_whitespace() || character == ','
            })
        }) {
            return Err(invalid("invalid no-proxy entry"));
        }
        result.insert("no-proxy".to_owned(), json!(proxy.no_proxy.join(",")));
    }
    Ok(result)
}

fn valid_name(domain: &str) -> io::Result<&str> {
    if domain.is_empty()
        || !domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
    {
        return Err(invalid("invalid resolver domain"));
    }
    Ok(domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsa_proto::guest::v1::HostAlias;

    #[test]
    fn applies_aliases_and_keeps_security_options() {
        let request = ConfigureNetwork {
            registries: Some(marsa_proto::guest::v1::RegistryConfig {
                mirrors: vec!["https://mirror.test".into()],
                insecure: vec!["registry.test:5000".into()],
            }),
            native_dns: None,
            dns_servers: vec!["1.1.1.1".into()],
            search_domains: vec!["corp.example".into()],
            host_aliases: vec![HostAlias {
                name: "host.docker.internal".into(),
                address: "192.0.2.1".into(),
            }],
            proxy: Some(Proxy {
                http_url: Some("http://proxy.example:3128".into()),
                ..Default::default()
            }),
        };
        let output = resolv_conf("", &request).unwrap();
        assert!(output.contains("nameserver 1.1.1.1\n"));
        assert!(output.contains("search corp.example\n"));
        assert_eq!(
            hosts_file("127.0.0.1 localhost\n", &request).unwrap(),
            "127.0.0.1 localhost\n192.0.2.1\thost.docker.internal\n"
        );
        let daemon = daemon_config(
            br#"{"live-restore":false,"seccomp-profile":"builtin"}"#,
            &request,
        )
        .unwrap()
        .unwrap();
        let daemon: Value = serde_json::from_slice(&daemon).unwrap();
        assert_eq!(daemon["live-restore"], false);
        assert_eq!(daemon["registry-mirrors"], json!(["https://mirror.test"]));
        assert_eq!(daemon["insecure-registries"], json!(["registry.test:5000"]));
        assert_eq!(daemon["seccomp-profile"], "builtin");
        assert_eq!(daemon["host-gateway-ips"], json!(["192.0.2.1"]));
        assert_eq!(daemon["proxies"]["http-proxy"], "http://proxy.example:3128");
    }

    #[test]
    fn rejects_proxy_errors_without_echoing_input() {
        let credentials = "private-user:private-password";
        let proxy = Proxy {
            http_url: Some(format!("file://{credentials}/proxy")),
            ..Default::default()
        };
        assert!(
            !docker_proxy(&proxy)
                .unwrap_err()
                .to_string()
                .contains(credentials)
        );
    }
}
