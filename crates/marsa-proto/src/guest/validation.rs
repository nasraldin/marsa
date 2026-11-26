//! Validate the shape and serialized size of each guest control message.

use std::net::IpAddr;

use super::v1::{
    CommandStatus, ConfigureNetwork, EgressStage, EngineHealthState, GuestEnvelope, HostEnvelope,
    PortAction, PortEvent, Transport, guest_envelope, host_envelope,
};
use super::{DEFER_ENGINE_START_MINOR, MAX_FRAME_BYTES, ProtocolError};

const KIB: usize = 1024;

pub(super) fn host(envelope: &HostEnvelope) -> Result<usize, ProtocolError> {
    use host_envelope::Body;

    let body = envelope
        .body
        .as_ref()
        .ok_or(ProtocolError::InvalidField("body"))?;
    check(
        (envelope.request_id == 0) == matches!(body, Body::HelloAck(_)),
        "request_id",
    )?;

    Ok(match body {
        Body::HelloAck(message) => {
            let version = message
                .protocol
                .ok_or(ProtocolError::InvalidField("protocol"))?;
            check(
                !message.defer_engine_start || version.minor >= DEFER_ENGINE_START_MINOR,
                "defer_engine_start",
            )?;
            64
        }
        Body::SetTime(message) => {
            check(message.nanoseconds < 1_000_000_000, "nanoseconds")?;
            64
        }
        Body::ConfigureNetwork(message) => {
            network(message)?;
            32 * KIB
        }
        Body::InstallCertificates(message) => {
            check(message.certificates_der.len() <= 128, "certificates_der")?;
            check(
                message
                    .certificates_der
                    .iter()
                    .all(|certificate| (1..=16 * KIB).contains(&certificate.len())),
                "certificate_der",
            )?;
            MAX_FRAME_BYTES
        }
        Body::Trim(_) | Body::Shutdown(_) | Body::StartEngine(_) => 32,
        Body::ProbeEgress(message) => {
            check(message.address_v4.len() == 4, "address_v4")?;
            32
        }
        Body::MountShare(message) => {
            tag(&message.tag)?;
            text(&message.guest_path, 4096, "guest_path")?;
            let mut segments = message.guest_path.split('/');
            let starts_at_root = segments.next() == Some("");
            check(
                starts_at_root && segments.all(|segment| !matches!(segment, "" | "." | "..")),
                "guest_path",
            )?;
            5 * KIB
        }
        Body::UnmountShare(message) => {
            tag(&message.tag)?;
            128
        }
    })
}

pub(super) fn guest(envelope: &GuestEnvelope) -> Result<usize, ProtocolError> {
    use guest_envelope::Body;

    let body = envelope
        .body
        .as_ref()
        .ok_or(ProtocolError::InvalidField("body"))?;
    check(
        (envelope.request_id != 0)
            == matches!(
                body,
                Body::TrimResult(_) | Body::CommandResult(_) | Body::EgressResult(_)
            ),
        "request_id",
    )?;

    Ok(match body {
        Body::Hello(message) => {
            check(message.protocol.is_some(), "protocol")?;
            text(&message.image_version, 128, "image_version")?;
            check(message.components.len() <= 32, "components")?;
            check(message.boot_timings.len() <= 32, "boot_timings")?;
            for component in &message.components {
                text(&component.name, 64, "component_name")?;
                text(&component.version, 128, "component_version")?;
            }
            for timing in &message.boot_timings {
                text(&timing.stage, 64, "boot_stage")?;
            }
            12 * KIB
        }
        Body::PortEvent(message) => {
            port_event(message)?;
            256
        }
        Body::EngineHealth(message) => {
            check(
                matches!(EngineHealthState::try_from(message.state), Ok(state) if state != EngineHealthState::Unspecified),
                "engine_health_state",
            )?;
            detail(&message.detail)?;
            3 * KIB
        }
        Body::Stats(message) => {
            check(
                message.memory_used_bytes <= message.memory_total_bytes,
                "memory_used_bytes",
            )?;
            check(
                message.data_disk_used_bytes <= message.data_disk_total_bytes,
                "data_disk_used_bytes",
            )?;
            128
        }
        Body::TrimResult(_) => 64,
        Body::EgressResult(message) => {
            check(
                matches!(EgressStage::try_from(message.stage), Ok(stage) if stage != EgressStage::Unspecified),
                "egress_stage",
            )?;
            32
        }
        Body::CommandResult(message) => {
            check(
                matches!(CommandStatus::try_from(message.status), Ok(status) if status != CommandStatus::Unspecified),
                "command_status",
            )?;
            detail(&message.detail)?;
            3 * KIB
        }
    })
}

fn network(message: &ConfigureNetwork) -> Result<(), ProtocolError> {
    check(message.dns_servers.len() <= 16, "dns_servers")?;
    for value in &message.dns_servers {
        ip_address(value, "dns_server")?;
    }
    check(message.search_domains.len() <= 16, "search_domains")?;
    for value in &message.search_domains {
        domain(value, "search_domain")?;
    }
    if let Some(native) = &message.native_dns {
        check(native.routes.len() <= 64, "dns_routes")?;
        for route in &native.routes {
            domain(&route.domain, "dns_route_domain")?;
            check(route.servers.len() <= 16, "dns_route_servers")?;
            check(route.port <= u32::from(u16::MAX), "dns_route_port")?;
            for server in &route.servers {
                ip_address(server, "dns_route_server")?;
            }
        }
    }
    check(message.host_aliases.len() <= 32, "host_aliases")?;
    for alias in &message.host_aliases {
        domain(&alias.name, "host_alias_name")?;
        ip_address(&alias.address, "host_alias_address")?;
    }
    if let Some(proxy) = &message.proxy {
        for url in proxy.http_url.iter().chain(&proxy.https_url) {
            text(url, 2048, "proxy_url")?;
            check(!url.chars().any(char::is_whitespace), "proxy_url")?;
        }
        check(proxy.no_proxy.len() <= 32, "no_proxy")?;
        for value in &proxy.no_proxy {
            text(value, 255, "no_proxy_entry")?;
            check(
                !value.contains(',') && !value.chars().any(char::is_whitespace),
                "no_proxy_entry",
            )?;
        }
    }
    Ok(())
}

fn port_event(message: &PortEvent) -> Result<(), ProtocolError> {
    check(
        matches!(
            PortAction::try_from(message.action),
            Ok(PortAction::Opened | PortAction::Closed)
        ),
        "port_action",
    )?;
    check(
        matches!(
            Transport::try_from(message.transport),
            Ok(Transport::Tcp | Transport::Udp)
        ),
        "transport",
    )?;
    check(
        message.container_id.len() == 64
            && message
                .container_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "container_id",
    )?;
    ip_address(&message.bind_address, "bind_address")?;
    check(
        (1..=65535).contains(&message.published_port),
        "published_port",
    )?;
    check(
        (1..=65535).contains(&message.container_port),
        "container_port",
    )
}

fn ip_address(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    check(value.len() <= 45 && value.parse::<IpAddr>().is_ok(), field)
}

fn domain(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    let name = value.strip_suffix('.').unwrap_or(value);
    check(
        name.len() <= 253
            && name.split('.').all(|label| {
                (1..=63).contains(&label.len())
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            }),
        field,
    )
}

fn tag(value: &str) -> Result<(), ProtocolError> {
    check(
        (1..=36).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)),
        "share_tag",
    )
}

fn text(value: &str, max_bytes: usize, field: &'static str) -> Result<(), ProtocolError> {
    check(
        (1..=max_bytes).contains(&value.len()) && !value.chars().any(char::is_control),
        field,
    )
}

fn detail(value: &str) -> Result<(), ProtocolError> {
    check(
        value.len() <= 2048
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t')),
        "detail",
    )
}

fn check(valid: bool, field: &'static str) -> Result<(), ProtocolError> {
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::InvalidField(field))
    }
}
