//! Conservative interpretation of macOS proxy and route information.

use std::{net::Ipv4Addr, str::Lines};

#[cfg(test)]
mod tests;

pub(super) fn direct_proxy_configuration(output: &[u8]) -> bool {
    let Ok(output) = std::str::from_utf8(output) else {
        return false;
    };
    let mut lines = output.lines();
    lines.next() == Some("<dictionary> {")
        && proxy_dictionary(&mut lines, 1).is_some()
        && lines.next().is_none()
}

fn proxy_dictionary(lines: &mut Lines<'_>, depth: usize) -> Option<()> {
    let indent = "  ".repeat(depth);
    let closing = format!("{}}}", "  ".repeat(depth - 1));
    loop {
        let line = lines.next()?;
        if line == closing {
            return Some(());
        }
        let (key, value) = line.strip_prefix(&indent)?.split_once(" : ")?;
        match (key, value) {
            ("__SCOPED__", "<dictionary> {") if depth == 1 => proxy_scopes(lines)?,
            ("ExceptionsList", "<array> {") => proxy_exceptions(lines, &indent)?,
            ("ExcludeSimpleHostnames" | "FTPPassive", "0" | "1")
            | (
                "HTTPEnable"
                | "HTTPSEnable"
                | "SOCKSEnable"
                | "ProxyAutoConfigEnable"
                | "ProxyAutoDiscoveryEnable"
                | "FTPEnable"
                | "GopherEnable"
                | "RTSPEnable"
                | "SMBEnable",
                "0",
            ) => {}
            // Supplemental and service policies, endpoint values, and newly introduced
            // keys must be understood before the comparator can make a direct request.
            _ => return None,
        }
    }
}

fn proxy_scopes(lines: &mut Lines<'_>) -> Option<()> {
    loop {
        let line = lines.next()?;
        if line == "  }" {
            return Some(());
        }
        let interface = line
            .strip_prefix("    ")?
            .strip_suffix(" : <dictionary> {")?;
        if !interface_name(interface) {
            return None;
        }
        proxy_dictionary(lines, 3)?;
    }
}

fn proxy_exceptions(lines: &mut Lines<'_>, indent: &str) -> Option<()> {
    let indent_entry = format!("{indent}  ");
    let closing = format!("{indent}}}");
    for (index, line) in lines.enumerate() {
        if line == closing {
            return Some(());
        }
        let (key, value) = line.strip_prefix(&indent_entry)?.split_once(" : ")?;
        if key.parse::<usize>().ok() != Some(index)
            || value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"*-_.:/%".contains(&byte))
        {
            return None;
        }
    }
    None
}

fn interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 16
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(super) fn route_signature(output: &str, target: Ipv4Addr) -> Option<String> {
    let (route, _) = output.split_once("\n recvpipe ")?;
    let mut lines = route.lines().map(str::trim_start);
    let destination = lines.next()?.strip_prefix("route to: ")?;
    if destination.parse::<Ipv4Addr>().ok()? != target {
        return None;
    }
    let destination = lines.next()?.strip_prefix("destination: ")?;
    if destination != "default" && destination.parse::<Ipv4Addr>().is_err() {
        return None;
    }
    let mut next = lines.next()?;
    if let Some(mask) = next.strip_prefix("mask: ") {
        if mask != "default" && mask.parse::<Ipv4Addr>().is_err() {
            return None;
        }
        next = lines.next()?;
    }
    let gateway = next.strip_prefix("gateway: ")?;
    if gateway.parse::<Ipv4Addr>().is_err()
        && gateway
            .strip_prefix("link#")
            .and_then(|index| index.parse::<u32>().ok())
            .is_none()
    {
        return None;
    }
    let interface = lines.next()?.strip_prefix("interface: ")?;
    if !interface_name(interface) || interface.starts_with("lo") {
        return None;
    }
    let flags = lines.next()?.strip_prefix("flags: <")?.strip_suffix('>')?;
    if !flags.split(',').any(|flag| flag == "UP")
        || flags.split(',').any(|flag| {
            !matches!(
                flag,
                "UP" | "GATEWAY"
                    | "HOST"
                    | "DYNAMIC"
                    | "MODIFIED"
                    | "DONE"
                    | "DELCLONE"
                    | "CLONING"
                    | "LLINFO"
                    | "STATIC"
                    | "PRCLONING"
                    | "WASCLONED"
                    | "PINNED"
                    | "IFSCOPE"
                    | "IFREF"
                    | "ROUTER"
                    | "GLOBAL"
            )
        })
        || lines.next().is_some()
    {
        return None;
    }
    // The kernel can clone a prefix route into a host route on the first
    // connection. Its destination, mask, and bookkeeping flags then change
    // while the next hop and interface for this target remain the same.
    Some(format!("{interface} {gateway}"))
}

pub(super) fn public_destination(ip: Ipv4Addr) -> bool {
    // Decline IANA special-purpose prefixes even when a specific anycast address is globally
    // reachable. The comparator only needs ordinary globally routed unicast destinations.
    !matches!(
        ip.octets(),
        [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [168, 63, 129, 16]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 0, 0 | 2, _]
            | [192, 31, 196, _]
            | [192, 52, 193, _]
            | [192, 88, 99, _]
            | [192, 168, ..]
            | [192, 175, 48, _]
            | [198, 18 | 19, ..]
            | [198, 51, 100, _]
            | [203, 0, 113, _]
    )
}

pub(super) fn verified_https_response(output: &[u8], target: Ipv4Addr) -> bool {
    let Ok(response) = std::str::from_utf8(output) else {
        return false;
    };
    let mut fields = response.split_ascii_whitespace();
    matches!(fields.next(), Some("200" | "401"))
        && fields.next().and_then(|ip| ip.parse().ok()) == Some(target)
        && fields.next() == Some("0")
        && fields.next().is_none()
}
