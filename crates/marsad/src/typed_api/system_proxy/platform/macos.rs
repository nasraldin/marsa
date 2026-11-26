//! Translate Apple's scoped dictionaries into a single Docker daemon policy.

use super::*;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, Default)]
struct Fixed {
    http: Option<String>,
    https: Option<String>,
    bypass: Vec<String>,
}

fn invalid() -> Status {
    Status::failed_precondition("invalid macOS proxy configuration | إعدادات وكيل macOS غير صالحة")
}

pub(super) fn resolve_snapshot(draft: &ProxySettings, bytes: &[u8]) -> Result<BootProxy, Status> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut lines = text.lines();
    if lines.next() != Some("<dictionary> {") {
        return Err(invalid());
    }
    let mut values = dictionary(&mut lines, 0)?;
    if lines.next().is_some() {
        return Err(invalid());
    }
    let scopes = values.remove("__SCOPED__");
    let global = fixed(&values, draft)?;
    if let Some(scopes) = scopes {
        let Node::Map(scopes) = scopes else {
            return Err(invalid());
        };
        for scope in scopes.into_values() {
            let Node::Map(scope) = scope else {
                return Err(invalid());
            };
            if fixed(&scope, draft)? != global {
                return Err(Status::failed_precondition(
                    "macOS interface proxy policies differ; choose a manual Docker proxy | تختلف سياسات الوكيل بين واجهات macOS. حدد وكيل Docker يدوياً",
                ));
            }
        }
    }
    BootProxy::resolved(
        draft,
        ProxySettings {
            http: global.http,
            https: global.https,
            ..Default::default()
        },
        global.bypass,
    )
    .map_err(|_| invalid())
}

enum Node<'a> {
    Scalar(&'a str),
    Array(Vec<&'a str>),
    Map(BTreeMap<&'a str, Node<'a>>),
}

fn dictionary<'a>(
    lines: &mut std::str::Lines<'a>,
    depth: usize,
) -> Result<BTreeMap<&'a str, Node<'a>>, Status> {
    if depth > 3 {
        return Err(unsupported());
    }
    let mut result = BTreeMap::new();
    let closing = format!("{}}}", "  ".repeat(depth));
    let indent = "  ".repeat(depth + 1);
    loop {
        let line = lines.next().ok_or_else(invalid)?;
        if line == closing {
            return Ok(result);
        }
        let (key, value) = line
            .strip_prefix(&indent)
            .and_then(|s| s.split_once(" : "))
            .ok_or_else(invalid)?;
        if result.len() >= 128 || key.is_empty() || key.bytes().any(|b| !b.is_ascii_graphic()) {
            return Err(invalid());
        }
        let node = match value {
            "<dictionary> {" => Node::Map(dictionary(lines, depth + 1)?),
            "<array> {" => {
                let mut entries = Vec::new();
                let end = format!("{indent}}}");
                loop {
                    let item = lines.next().ok_or_else(invalid)?;
                    if item == end {
                        break;
                    }
                    let prefix = format!("{indent}  {} : ", entries.len());
                    let entry = item.strip_prefix(&prefix).ok_or_else(invalid)?;
                    if entries.len() >= 32 || entry.len() > 255 {
                        return Err(unsupported());
                    }
                    entries.push(entry);
                }
                Node::Array(entries)
            }
            value if value.len() <= 2048 && !value.chars().any(char::is_control) => {
                Node::Scalar(value)
            }
            _ => return Err(invalid()),
        };
        if result.insert(key, node).is_some() {
            return Err(invalid());
        }
    }
}

fn fixed(values: &BTreeMap<&str, Node<'_>>, draft: &ProxySettings) -> Result<Fixed, Status> {
    for (key, value) in values {
        match (*key, value) {
            ("ExceptionsList", Node::Array(_))
            | ("HTTPProxy" | "HTTPSProxy" | "HTTPPort" | "HTTPSPort", Node::Scalar(_)) => (),
            (
                "HTTPEnable" | "HTTPSEnable" | "ExcludeSimpleHostnames" | "FTPPassive",
                Node::Scalar("0" | "1"),
            ) => (),
            (
                "ProxyAutoConfigEnable"
                | "ProxyAutoDiscoveryEnable"
                | "SOCKSEnable"
                | "FTPEnable"
                | "SMBEnable"
                | "RTSPEnable"
                | "GopherEnable",
                Node::Scalar("0"),
            ) => (),
            // Unknown authentication, PAC, supplemental and service policies must not
            // silently become a direct connection or a different static proxy.
            _ => return Err(unsupported()),
        }
    }
    let http = endpoint(values, "HTTP", draft.http.as_ref())?;
    let https = endpoint(values, "HTTPS", draft.https.as_ref())?;
    let mut bypass = Vec::new();
    let inherits_proxy =
        draft.http.is_none() && http.is_some() || draft.https.is_none() && https.is_some();
    if inherits_proxy {
        if matches!(
            values.get("ExcludeSimpleHostnames"),
            Some(Node::Scalar("1"))
        ) {
            return Err(unsupported());
        }
        if let Some(Node::Array(entries)) = values.get("ExceptionsList") {
            for entry in entries {
                // Docker already bypasses localhost. An exact DNS name on macOS cannot
                // be represented: Docker would exempt every subdomain as well.
                if entry.eq_ignore_ascii_case("localhost") {
                    continue;
                }
                bypass.push(normalize_bypass(entry).ok_or_else(unsupported)?);
            }
        }
        bypass.sort();
        bypass.dedup();
    }
    Ok(Fixed {
        http,
        https,
        bypass,
    })
}

fn endpoint(
    values: &BTreeMap<&str, Node<'_>>,
    scheme: &str,
    manual: Option<&String>,
) -> Result<Option<String>, Status> {
    if let Some(manual) = manual {
        return Ok(Some(manual.clone()));
    }
    let scalar = |key: &str| match values.get(key) {
        Some(Node::Scalar(value)) => Some(*value),
        _ => None,
    };
    let host = scalar(&format!("{scheme}Proxy"));
    match scalar(&format!("{scheme}Enable")) {
        Some("0") => return Ok(None),
        None if host.is_none() => return Ok(None),
        Some("1") => (),
        _ => return Err(invalid()),
    }
    let host = host.ok_or_else(invalid)?;
    let port = scalar(&format!("{scheme}Port"))
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .ok_or_else(invalid)?;
    let hostname = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let host = if let Ok(ip) = hostname.parse::<IpAddr>() {
        if ip.is_loopback()
            || ip.is_unspecified()
            || matches!(ip, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some())
        {
            return Err(unsupported());
        }
        match ip {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        }
    } else {
        let name = host.trim_end_matches('.').to_ascii_lowercase();
        if name == "localhost"
            || name.ends_with(".localhost")
            || !domain(&name)
            || name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            return Err(unsupported());
        }
        name
    };
    // The secure-web-proxy setting selects HTTPS destinations; it uses HTTP CONNECT.
    Ok(Some(format!("http://{host}:{port}")))
}

fn normalize_bypass(entry: &str) -> Option<String> {
    if let Some(suffix) = entry.strip_prefix("*.") {
        return domain(suffix).then(|| format!(".{}", suffix.to_ascii_lowercase()));
    }
    if let Some((network, prefix)) = entry.split_once('/') {
        // SystemConfiguration commonly reports link-local networks as 169.254/16.
        // Apple pads missing octets with zero. Unlike Go it does not mask host bits.
        let mut octets = [0u8; 4];
        for (index, part) in network.split('.').enumerate() {
            if part.len() > 1 && part.starts_with('0') {
                return None;
            }
            *octets.get_mut(index)? = part.parse().ok()?;
        }
        let network = std::net::Ipv4Addr::from(octets);
        let prefix: u8 = prefix.parse().ok()?;
        if prefix == 0 || prefix > 32 || u32::from(network) & !(u32::MAX << (32 - prefix)) != 0 {
            return None;
        }
        return Some(format!("{network}/{prefix}"));
    }
    valid_bypass(entry).then(|| entry.to_ascii_lowercase())
}

#[cfg(test)]
mod tests;
