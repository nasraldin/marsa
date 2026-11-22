//! Report the daemon's current network attachment and observed addresses.

use std::{fmt::Write as _, net::Ipv4Addr};

use marsa_client::{
    DaemonEndpoint,
    api::{NetworkMode, NetworkStatus, NetworkStatusRequest},
};
use serde_json::json;

use crate::{Failure, Outcome, command::Language, engine::connection_failure};

pub(super) async fn status(language: Language) -> Result<Outcome, Failure> {
    let client = DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)?;
    let result = client
        .network()
        .status(NetworkStatusRequest {})
        .await
        .map_err(|_| Failure::Operation)?
        .into_inner();
    Ok(report(&result, language))
}

fn report(status: &NetworkStatus, language: Language) -> Outcome {
    let configured = status.configured_mode();
    let active = status.active_mode();
    let mut text = format!(
        "{} {}\n{} {}\n",
        language.text("Configured mode", "الوضع المضبوط"),
        mode_text(configured, language),
        language.text("Active mode", "الوضع النشط"),
        mode_text(active, language),
    );
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
            "{} \u{2068}{}\u{2069}",
            language.text("Connectivity check", "فحص الاتصال"),
            reason.escape_debug()
        );
    }
    if let Some(guest) = guest {
        let _ = writeln!(
            text,
            "{} \u{2068}{guest}\u{2069}",
            language.text("Guest IPv4", "عنوان IPv4 للضيف")
        );
    }
    if let Some(gateway) = gateway {
        let _ = writeln!(
            text,
            "{} \u{2068}{gateway}\u{2069}",
            language.text("Gateway IPv4", "عنوان IPv4 للبوابة")
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

fn mode_text(mode: NetworkMode, language: Language) -> &'static str {
    match mode {
        NetworkMode::Auto => language.text("Automatic", "تلقائي"),
        NetworkMode::Nat => language.text("Native NAT", "NAT الأصلي"),
        NetworkMode::Userspace => language.text("User-space", "وضع المستخدم"),
        NetworkMode::Unspecified => language.text("Unavailable", "غير متاح"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_guest_cannot_present_stale_addresses() {
        let status = NetworkStatus {
            configured_mode: NetworkMode::Auto.into(),
            guest_address: Some("192.168.127.2".into()),
            ..Default::default()
        };
        let Outcome::Data { document, .. } = report(&status, Language::English) else {
            panic!("expected network data");
        };
        assert_eq!(document["configured_mode"], "auto");
        assert!(document["active_mode"].is_null());
        assert!(document["guest_address"].is_null());
    }
}
