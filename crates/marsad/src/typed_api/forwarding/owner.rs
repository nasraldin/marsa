//! Resolve a local bind conflict only when the operating system reveals its owner.

use marsa_proto::host::v1::ForwardedPortFailure;

pub(super) async fn identify(failures: &mut [ForwardedPortFailure]) {
    #[cfg(target_os = "macos")]
    for failure in failures
        .iter_mut()
        .filter(|failure| failure.message.contains("already in use"))
        .take(32)
    {
        failure.owner = macos_owner(failure).await;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = failures;
}

#[cfg(target_os = "macos")]
async fn macos_owner(failure: &ForwardedPortFailure) -> Option<String> {
    use marsa_proto::host::v1::PortTransport;
    use std::time::Duration;
    let protocol = if failure.transport() == PortTransport::Tcp {
        "TCP"
    } else {
        "UDP"
    };
    let family = if failure.host_address.contains(':') {
        6
    } else {
        4
    };
    let selector = format!("-i{family}{protocol}:{}", failure.host_port);
    let mut arguments = vec!["-nP", "-a", &selector, "-Fpcn"];
    if protocol == "TCP" {
        arguments.push("-sTCP:LISTEN");
    }
    let bytes = crate::platform_command::output(
        "/usr/sbin/lsof",
        &arguments,
        Duration::from_secs(1),
        16_384,
    )
    .await?;
    let mut pid = "";
    let mut name = "";
    let exact = if family == 6 {
        format!("[{}]", failure.host_address)
    } else {
        failure.host_address.clone()
    };
    for line in std::str::from_utf8(&bytes).ok()?.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value;
            name = "";
        }
        if let Some(value) = line.strip_prefix('c') {
            name = value;
        }
        if let Some(endpoint) = line.strip_prefix('n') {
            let local = endpoint
                .split_once("->")
                .map_or(endpoint, |(local, _)| local);
            let Some((ip, port)) = local.rsplit_once(':') else {
                continue;
            };
            if port.parse::<u32>().ok() == Some(failure.host_port)
                && (ip == "*"
                    || ip == exact
                    || failure.host_address == "0.0.0.0"
                    || failure.host_address == "::")
                && !name.is_empty()
                && pid.parse::<u32>().is_ok()
            {
                return Some(format!(
                    "{} ({pid})",
                    name.chars()
                        .filter(|c| !c.is_control())
                        .take(64)
                        .collect::<String>()
                ));
            }
        }
    }
    None
}
