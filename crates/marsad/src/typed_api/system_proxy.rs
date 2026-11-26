//! Resolve a reviewable, fixed Docker proxy from a bounded macOS policy snapshot.
//! Docker has one process-wide policy: differing interface or service policies
//! cannot be flattened without changing which destinations bypass the proxy.

use std::net::IpAddr;

use marsa_core::settings::ProxySettings;
use tonic::Status;

use super::proxy_policy::BootProxy;

mod platform;

fn unsupported() -> Status {
    Status::failed_precondition(
        "the system proxy cannot be represented by Docker; choose explicit manual proxy settings | لا يستطيع Docker تمثيل وكيل النظام. حدد إعدادات الوكيل يدوياً",
    )
}

#[tonic::async_trait]
pub(super) trait Discovery: Send + Sync {
    async fn discover(&self, draft: &ProxySettings) -> Result<BootProxy, Status>;
}

pub(super) struct Platform;

pub(super) async fn resolve(
    draft: &ProxySettings,
    discovery: &dyn Discovery,
) -> Result<BootProxy, Status> {
    if !draft.follow_system || (draft.http.is_some() && draft.https.is_some()) {
        return BootProxy::from_draft(draft).map_err(|_| unsupported());
    }
    discovery.discover(draft).await
}

impl super::TypedApi {
    pub(super) async fn observe_system_proxy(&self) {
        let Some(runtime) = &self.proxy else { return };
        let _edit = self.inner.edit.lock().await;
        let (settings, _) = self.inner.events.settings();
        let draft = &settings.network.proxy;
        let pending = if draft.follow_system && (draft.http.is_none() || draft.https.is_none()) {
            // Retain the approved boot policy on discovery failure. The preview
            // provides a concrete reason before the user can authorize any change.
            resolve(draft, self.proxy_discovery.as_ref())
                .await
                .map_or(true, |snapshot| snapshot != runtime.approved())
        } else {
            false
        };
        self.inner.events.system_proxy_pending(pending);
    }
}

pub(super) fn valid_bypass(entry: &str) -> bool {
    if entry.is_empty() || entry.len() > 255 {
        return false;
    }
    if entry.parse::<IpAddr>().is_ok() {
        return true;
    }
    if let Some(suffix) = entry.strip_prefix('.') {
        return domain(suffix);
    }
    let Some((network, prefix)) = entry.split_once('/') else {
        return false;
    };
    matches!((network.parse::<IpAddr>(), prefix.parse::<u8>()), (Ok(ip), Ok(bits)) if bits <= if ip.is_ipv4() { 32 } else { 128 })
}

fn domain(name: &str) -> bool {
    name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
