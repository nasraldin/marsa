//! Read the network attachment and stage a mode for the next VM activation.

use std::{fmt::Write as _, net::Ipv4Addr};

use marsa_client::{
    DaemonEndpoint, MarsaClient,
    api::{NetworkMode, NetworkStatus, NetworkStatusRequest, SetNetworkModeRequest},
    l10n::keys,
};
use serde_json::json;

use crate::{
    Failure, Outcome, command,
    engine::{connection_failure, request_id},
};

pub(super) async fn status() -> Result<Outcome, Failure> {
    Ok(report(&read(&connect().await?).await?))
}

pub(super) async fn mode(requested: Option<&str>) -> Result<Outcome, Failure> {
    let requested = requested.map(parse_mode).transpose()?;
    let client = connect().await?;
    let status = match requested {
        Some(mode) => client
            .network()
            .set_mode(SetNetworkModeRequest {
                request_id: request_id()?,
                mode: mode.into(),
            })
            .await
            .map_err(|_| Failure::Operation)?
            .into_inner(),
        None => read(&client).await?,
    };
    Ok(report_mode(&status))
}

async fn connect() -> Result<MarsaClient, Failure> {
    DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)
}

async fn read(client: &MarsaClient) -> Result<NetworkStatus, Failure> {
    client
        .network()
        .status(NetworkStatusRequest {})
        .await
        .map(|response| response.into_inner())
        .map_err(|_| Failure::Operation)
}

fn parse_mode(value: &str) -> Result<NetworkMode, Failure> {
    match value {
        "auto" => Ok(NetworkMode::Auto),
        "nat" => Ok(NetworkMode::Nat),
        "userspace" => Ok(NetworkMode::Userspace),
        _ => Err(Failure::Usage),
    }
}

fn report_mode(status: &NetworkStatus) -> Outcome {
    let mut text = mode_lines(status);
    let pending = match status.restart_pending {
        Some(true) => command::label(keys::CLI_NETWORK_RESTART_PENDING),
        Some(false) => command::label(keys::CLI_NETWORK_RESTART_CURRENT),
        None => command::label(keys::CLI_NETWORK_RESTART_UNKNOWN),
    };
    let _ = writeln!(text, "{pending}");
    Outcome::Data {
        document: json!({
            "schema_version": 1,
            "kind": "network_mode",
            "configured_mode": mode_id(status.configured_mode()),
            "active_mode": mode_id(status.active_mode()),
            "restart_pending": status.restart_pending,
        }),
        text,
    }
}

fn report(status: &NetworkStatus) -> Outcome {
    let configured = status.configured_mode();
    let active = status.active_mode();
    let mut text = mode_lines(status);
    let reason = status.reason.as_deref();
    let (guest, gateway) = match active {
        NetworkMode::Nat | NetworkMode::Userspace => (
            address(status.guest_address.as_deref()),
            address(status.gateway_address.as_deref()),
        ),
        _ => (None, None),
    };
    if let Some(reason) = reason {
        let _ = writeln!(
            text,
            "{}",
            command::message(
                keys::CLI_NETWORK_CONNECTIVITY,
                &[("value", &command::technical(reason))]
            )
        );
    }
    if let Some(guest) = guest {
        let _ = writeln!(
            text,
            "{}",
            command::message(
                keys::CLI_NETWORK_GUEST_ADDRESS,
                &[("value", &command::technical(&guest.to_string()))]
            )
        );
    }
    if let Some(gateway) = gateway {
        let _ = writeln!(
            text,
            "{}",
            command::message(
                keys::CLI_NETWORK_GATEWAY_ADDRESS,
                &[("value", &command::technical(&gateway.to_string()))]
            )
        );
    }
    Outcome::Data {
        document: json!({
            "schema_version": 1,
            "kind": "network_status",
            "configured_mode": mode_id(configured),
            "active_mode": mode_id(active),
            "reason": reason,
            "guest_address": guest,
            "gateway_address": gateway,
        }),
        text,
    }
}

fn mode_lines(status: &NetworkStatus) -> String {
    format!(
        "{}\n{}\n",
        command::message(
            keys::CLI_NETWORK_CONFIGURED_MODE,
            &[("mode", mode_text(status.configured_mode()))]
        ),
        command::message(
            keys::CLI_NETWORK_ACTIVE_MODE,
            &[("mode", mode_text(status.active_mode()))]
        ),
    )
}

fn address(value: Option<&str>) -> Option<Ipv4Addr> {
    value?.parse().ok()
}

fn mode_id(mode: NetworkMode) -> Option<&'static str> {
    match mode {
        NetworkMode::Auto => Some("auto"),
        NetworkMode::Nat => Some("nat"),
        NetworkMode::Userspace => Some("userspace"),
        NetworkMode::Unspecified => None,
    }
}

fn mode_text(mode: NetworkMode) -> &'static str {
    match mode {
        NetworkMode::Auto => command::label(keys::CLI_NETWORK_AUTOMATIC),
        NetworkMode::Nat => command::label(keys::CLI_NETWORK_NAT),
        NetworkMode::Userspace => command::label(keys::CLI_NETWORK_USERSPACE),
        NetworkMode::Unspecified => command::label(keys::CLI_NETWORK_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_pin_can_match_the_active_attachment() {
        let status = NetworkStatus {
            configured_mode: NetworkMode::Userspace.into(),
            active_mode: NetworkMode::Userspace.into(),
            restart_pending: Some(true),
            ..Default::default()
        };
        let Outcome::Data { document, text } = report_mode(&status) else {
            panic!("expected a network mode response");
        };
        assert_eq!(document["kind"], "network_mode");
        assert_eq!(document["configured_mode"], "userspace");
        assert_eq!(document["active_mode"], "userspace");
        assert_eq!(document["restart_pending"], true);
        assert!(text.contains("Restart pending."));
    }

    #[test]
    fn fallback_and_unobserved_boot_policy_keep_distinct_values() {
        for pending in [Some(false), None] {
            let status = NetworkStatus {
                configured_mode: NetworkMode::Auto.into(),
                active_mode: NetworkMode::Userspace.into(),
                restart_pending: pending,
                ..Default::default()
            };
            let Outcome::Data { document, .. } = report_mode(&status) else {
                panic!("expected a network mode response");
            };
            assert_eq!(document["configured_mode"], "auto");
            assert_eq!(document["active_mode"], "userspace");
            assert_eq!(document["restart_pending"], json!(pending));
        }
    }

    #[test]
    fn a_stopped_guest_cannot_present_stale_addresses() {
        let status = NetworkStatus {
            configured_mode: NetworkMode::Auto.into(),
            guest_address: Some("192.168.127.2".into()),
            ..Default::default()
        };
        let Outcome::Data { document, .. } = report(&status) else {
            panic!("expected network data");
        };
        assert_eq!(document["configured_mode"], "auto");
        assert!(document["active_mode"].is_null());
        assert!(document["guest_address"].is_null());
    }
}
