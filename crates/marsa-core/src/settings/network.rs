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
    /// Permit wildcard Docker publications to listen on LAN interfaces.
    pub allow_lan: bool,
    /// Manual DNS server addresses. An empty list follows the host resolver.
    pub dns: Vec<IpAddr>,
    /// Ordered Docker Hub mirror URLs. Empty uses Docker's upstream default.
    pub registry_mirrors: Vec<String>,
    /// Registry authorities or CIDRs explicitly allowed to use insecure transport.
    pub insecure_registries: Vec<String>,
    /// Host proxy overrides and container proxy injection policy.
    pub proxy: ProxySettings,
}

impl NetworkSettings {
    /// Validate the registry options shared by persisted settings and guest control.
    pub fn validate_registries(
        mirrors: &[String],
        insecure: &[String],
    ) -> Result<(), SettingsError> {
        if mirrors.len() > 16 || mirrors.iter().any(|value| !valid_mirror(value)) {
            return Err(SettingsError::invalid(
                "network.registry_mirrors",
                "expected up to 16 HTTP or HTTPS mirrors without credentials, fragments or queries",
            ));
        }
        if insecure.len() > 32 || insecure.iter().any(|value| !valid_insecure_registry(value)) {
            return Err(SettingsError::invalid(
                "network.insecure_registries",
                "expected up to 32 registry authorities or IP networks",
            ));
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), SettingsError> {
        if self.dns.len() > 16 {
            return Err(SettingsError::invalid(
                "network.dns",
                "allows up to 16 resolver addresses",
            ));
        }
        Self::validate_registries(&self.registry_mirrors, &self.insecure_registries)?;
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

fn valid_registry_authority(value: &str) -> bool {
    let Ok(authority) = value.parse::<http::uri::Authority>() else {
        return false;
    };
    !value.contains('@')
        && valid_proxy_host(authority.host())
        && (authority.as_str() == authority.host() || authority.port_u16().is_some_and(|p| p > 0))
}

fn valid_mirror(value: &str) -> bool {
    if value.len() > 1024 || value.contains(['#', '?']) {
        return false;
    }
    let Ok(uri) = value.parse::<Uri>() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https"))
        && uri
            .authority()
            .is_some_and(|authority| valid_registry_authority(authority.as_str()))
}

fn valid_insecure_registry(value: &str) -> bool {
    if value.len() > 260 {
        return false;
    }
    if let Some((address, prefix)) = value.split_once('/') {
        let Ok(address) = address.parse::<IpAddr>() else {
            return false;
        };
        return !prefix.is_empty()
            && prefix.bytes().all(|b| b.is_ascii_digit())
            && prefix
                .parse::<u8>()
                .is_ok_and(|prefix| prefix <= if address.is_ipv4() { 32 } else { 128 });
    }
    value.parse::<IpAddr>().is_ok() || valid_registry_authority(value)
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

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn registry_settings_validate_mirrors_and_explicit_insecure_destinations() {
        let mut settings = NetworkSettings {
            registry_mirrors: vec!["https://mirror.example.test/cache".into()],
            insecure_registries: vec!["[::1]:5000".into(), "192.0.2.0/24".into()],
            ..Default::default()
        };
        assert!(settings.validate().is_ok());
        for invalid in [
            "https://id:pw@mirror.test",
            "https://mirror.test/?token",
            "file:///x",
        ] {
            settings.registry_mirrors = vec![invalid.into()];
            assert!(settings.validate().is_err());
        }
        settings.registry_mirrors.clear();
        for invalid in ["https://registry.test", "foo:99999", "10.0.0.0/33", "a@b"] {
            settings.insecure_registries = vec![invalid.into()];
            assert!(settings.validate().is_err());
        }
    }
}
