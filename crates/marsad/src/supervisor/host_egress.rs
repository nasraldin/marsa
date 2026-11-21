//! Host evidence for direct IPv4 egress to the Docker registry.

use std::{fmt, net::Ipv4Addr};

#[cfg(target_os = "macos")]
mod platform;
#[cfg(target_os = "macos")]
mod policy;

/// TLS-verified registry IPv4 address and the host policy used to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) ip: Ipv4Addr,
    policy: Option<PolicySnapshot>,
}

#[cfg(test)]
impl Target {
    pub(super) fn fixture(ip: Ipv4Addr) -> Self {
        Self { ip, policy: None }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PolicySnapshot {
    route: String,
    proxy: Vec<u8>,
}

impl fmt::Debug for PolicySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PolicySnapshot")
            .finish_non_exhaustive()
    }
}

/// Returns a registry target verified under the current direct host policy.
pub(super) async fn verified_target() -> Option<Target> {
    #[cfg(target_os = "macos")]
    {
        platform::verified_target().await
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_includes_route_and_proxy_without_exposing_them_in_debug_output() {
        let ip = Ipv4Addr::new(35, 174, 73, 84);
        let target = |route: &str, proxy: &[u8]| Target {
            ip,
            policy: Some(PolicySnapshot {
                route: route.to_owned(),
                proxy: proxy.to_owned(),
            }),
        };
        let original = target("private route", b"private proxy");
        assert_eq!(original, target("private route", b"private proxy"));
        assert_ne!(original, target("new route", b"private proxy"));
        assert_ne!(original, target("private route", b"new proxy"));
        assert_ne!(original, Target::fixture(ip));
        assert!(!format!("{original:?}").contains("private"));
    }
}
