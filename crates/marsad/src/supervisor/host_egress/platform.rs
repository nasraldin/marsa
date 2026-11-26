//! Read macOS network policy without changing proxy, resolver, or routing state.

use std::{
    env,
    ffi::OsStr,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use tokio::{net::lookup_host, time::timeout};

use super::{
    PolicySnapshot, Target,
    policy::{
        direct_proxy_configuration, public_destination, route_signature, verified_https_response,
    },
};

const REGISTRY: &str = "registry-1.docker.io";
const DNS_NAME: &str = "registry-1.docker.io.";
const REGISTRY_URL: &str = "https://registry-1.docker.io/v2/";
const LOCAL_COMMAND_LIMIT: Duration = Duration::from_secs(1);
const HTTPS_COMMAND_LIMIT: Duration = Duration::from_secs(4);
const MAX_COMMAND_OUTPUT: u64 = 8192;
const MAX_RESOLVER_ANSWERS: usize = 32;

pub(super) async fn verified_target() -> Option<Target> {
    compare()
        .await
        .inspect_err(|stage| {
            tracing::debug!(stage, "automatic boot host comparison unavailable");
        })
        .ok()
}

async fn compare() -> Result<Target, &'static str> {
    if env::vars_os().any(|(name, _)| requests_custom_policy(&name)) {
        return Err("environment_policy");
    }

    let proxy = proxy_configuration().await.ok_or("proxy")?;
    let ip = registry_ipv4().await.ok_or("dns")?;
    let (before, current_proxy) = tokio::join!(route_to(ip), proxy_configuration());
    let before = before.ok_or("route")?;
    if current_proxy.ok_or("proxy")? != proxy {
        return Err("proxy_changed");
    }

    probe_registry(ip).await.ok_or("https")?;

    let (after, current_proxy) = tokio::join!(route_to(ip), proxy_configuration());
    if after.ok_or("route")? != before {
        return Err("route_changed");
    }
    if current_proxy.ok_or("proxy")? != proxy {
        return Err("proxy_changed");
    }
    Ok(Target {
        ip,
        policy: Some(PolicySnapshot {
            route: before,
            proxy,
        }),
    })
}

fn requests_custom_policy(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.to_ascii_lowercase().ends_with("_proxy")
        || matches!(
            name.as_ref(),
            "CURL_CA_BUNDLE" | "SSL_CERT_FILE" | "SSL_CERT_DIR"
        )
}

async fn registry_ipv4() -> Option<Ipv4Addr> {
    // Pass the absolute DNS name to the system resolver to preserve split DNS policy
    // without trying a local search suffix if the exact name does not resolve.
    let addresses = timeout(LOCAL_COMMAND_LIMIT, lookup_host((DNS_NAME, 443)))
        .await
        .ok()?
        .ok()?;
    registry_address(addresses)
}

fn registry_address(addresses: impl IntoIterator<Item = SocketAddr>) -> Option<Ipv4Addr> {
    let mut preferred = None;
    for (index, address) in addresses.into_iter().enumerate() {
        if index == MAX_RESOLVER_ANSWERS {
            return None;
        }
        if let SocketAddr::V4(address) = address {
            let candidate = *address.ip();
            if !public_destination(candidate) {
                return None;
            }
            preferred =
                Some(preferred.map_or(candidate, |previous: Ipv4Addr| previous.min(candidate)));
        }
    }
    preferred
}

async fn proxy_configuration() -> Option<Vec<u8>> {
    // Apple scutil omits scoped, service, and supplemental dictionaries without -d.
    let output =
        command_output("/usr/sbin/scutil", &["-d", "--proxy"], LOCAL_COMMAND_LIMIT).await?;
    direct_proxy_configuration(&output).then_some(output)
}

async fn route_to(target: Ipv4Addr) -> Option<String> {
    let output = command_output(
        "/sbin/route",
        &["-n", "get", "-inet", &target.to_string()],
        LOCAL_COMMAND_LIMIT,
    )
    .await?;
    let output = std::str::from_utf8(&output).ok()?;
    route_signature(output, target)
}

async fn probe_registry(target: Ipv4Addr) -> Option<()> {
    // Pin the verified resolver answer while retaining the URL hostname for TLS. -q
    // must be the first argument so curl cannot apply a user configuration file.
    let output = command_output(
        "/usr/bin/curl",
        &[
            "-q",
            "--ipv4",
            "--proto",
            "=https",
            "--silent",
            "--connect-timeout",
            "2",
            "--max-time",
            "3",
            "--max-filesize",
            "4096",
            "--output",
            "/dev/null",
            "--user-agent",
            "",
            "--resolve",
            &format!("{REGISTRY}:443:{target}"),
            "--write-out",
            "%{http_code} %{remote_ip} %{ssl_verify_result}\n",
            "--url",
            REGISTRY_URL,
        ],
        HTTPS_COMMAND_LIMIT,
    )
    .await?;
    verified_https_response(&output, target).then_some(())
}

async fn command_output(path: &str, arguments: &[&str], limit: Duration) -> Option<Vec<u8>> {
    crate::platform_command::output(path, arguments, limit, MAX_COMMAND_OUTPUT).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declines_inherited_proxy_and_trust_overrides() {
        for name in [
            "http_proxy",
            "HTTPS_PROXY",
            "no_proxy",
            "ALL_Proxy",
            "SSL_CERT_DIR",
        ] {
            assert!(requests_custom_policy(OsStr::new(name)));
        }
        assert!(!requests_custom_policy(OsStr::new("PATH")));
    }

    #[test]
    fn uses_the_same_public_ipv4_address_in_any_answer_order() {
        let first = SocketAddr::from(([35, 174, 73, 84], 443));
        let second = SocketAddr::from(([44, 208, 254, 194], 443));
        let ipv6 = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 443));
        for answers in [[first, ipv6, second], [second, first, ipv6]] {
            assert_eq!(
                registry_address(answers),
                Some(Ipv4Addr::new(35, 174, 73, 84))
            );
        }
        assert_eq!(registry_address([ipv6]), None);
        assert_eq!(
            registry_address([first, SocketAddr::from(([10, 0, 0, 1], 443))]),
            None
        );
        assert_eq!(registry_address([first; MAX_RESOLVER_ANSWERS + 1]), None);
    }
}
