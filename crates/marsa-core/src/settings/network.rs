//! Network selection and proxy overrides that can be stored without credentials.

use std::net::{IpAddr, Ipv6Addr};

use http::Uri;
use serde::{Deserialize, Serialize};

use crate::error::SettingsError;

/// How the daemon selects guest networking.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// Prefer native NAT and fall back when probes detect blocked egress.
    #[default]
    Auto,
    /// Always use the host's NAT attachment.
    Nat,
    /// Always use the user-space network stack.
    Userspace,
}

/// Guest network selection and overrides to the host configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkSettings {
    /// Guest egress configuration to request.
    pub mode: NetworkMode,
    /// Manual DNS server addresses. An empty list follows the host resolver.
    pub dns: Vec<IpAddr>,
    /// Host proxy overrides and container proxy injection policy.
    pub proxy: ProxySettings,
}

impl NetworkSettings {
    pub(super) fn validate(&self) -> Result<(), SettingsError> {
        if self.dns.len() > 16 {
            return Err(SettingsError::invalid(
                "network.dns",
                "allows up to 16 resolver addresses",
            ));
        }
        validate_proxy("network.proxy.http", self.proxy.http.as_deref())?;
        validate_proxy("network.proxy.https", self.proxy.https.as_deref())?;
        Ok(())
    }
}

/// Overrides for HTTP and HTTPS proxy discovery.
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxySettings {
    /// HTTP traffic proxy URL. Omitting it follows the corresponding system setting.
    pub http: Option<String>,
    /// HTTPS traffic proxy URL. Omitting it follows the corresponding system setting.
    pub https: Option<String>,
    /// Inject proxy configuration into new containers when explicitly enabled.
    pub inject_into_containers: bool,
}

impl std::fmt::Debug for ProxySettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxySettings")
            .field("has_http_override", &self.http.is_some())
            .field("has_https_override", &self.https.is_some())
            .field("inject_into_containers", &self.inject_into_containers)
            .finish()
    }
}

fn validate_proxy(field: &'static str, endpoint: Option<&str>) -> Result<(), SettingsError> {
    let Some(endpoint) = endpoint else {
        return Ok(());
    };
    let invalid = || {
        SettingsError::invalid(
            field,
            "must be an HTTP or HTTPS proxy URL without credentials, a path, query, or fragment",
        )
    };

    let uri: Uri = endpoint.parse().map_err(|_| invalid())?;
    let authority = uri.authority().ok_or_else(invalid)?;
    let host = authority.host();

    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || !valid_proxy_host(host)
        || authority.as_str().contains('@')
        || uri.path() != "/"
        || uri.query().is_some()
        || endpoint.contains('#')
    {
        return Err(invalid());
    }
    if authority.as_str() != host && !uri.port_u16().is_some_and(|port| port > 0) {
        return Err(invalid());
    }
    Ok(())
}

fn valid_proxy_host(host: &str) -> bool {
    if let Some(ip) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        return ip.parse::<Ipv6Addr>().is_ok();
    }

    let host = host.strip_suffix('.').unwrap_or(host);
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}
