//! Inspect only the Docker operations that can name host-backed binds or the reserved plugin.

mod channel;

use base64::{Engine, engine::general_purpose::STANDARD};
use http::Uri;
use marsa_core::docker_mounts::{MAX_CREATE_BYTES, sources};
use marsa_proto::guest::MAX_BIND_AUTHORIZATION_SOURCES;
use percent_encoding::percent_decode_str;
use serde::Deserialize;

pub(crate) use channel::Policy;

// In Moby 29.10, managed plugin lookup rejects this uppercase, single-component
// repository name and falls through to the legacy socket. No generated plugin ID
// can equal it, and Disable only removes authorization plugins resolved via the
// managed store. Keep the spelling in guest/rootfs/daemon.json identical.
pub(crate) const PLUGIN: &str = "MarsaBindAuthorization";

const INVALID_CREATE: &str = "Docker mount configuration could not be checked. Send a valid application/json container create request. | تعذّر التحقق من إعدادات التركيب. أرسل طلب إنشاء حاوية صالحًا بتنسيق application/json.";
const INVALID_CALLBACK: &str =
    "Marsa could not verify this Docker request. | تعذّر على مارسا التحقق من طلب دوكر.";
const RESERVED_PLUGIN: &str =
    "Marsa manages this Docker authorization plugin. | تدير مارسا ملحق التفويض هذا في دوكر.";
const POLICY_UNAVAILABLE: &str = "Marsa could not check host file sharing. Try again once Marsa is running. | تعذّر على مارسا التحقق من مشاركة ملفات المضيف. أعد المحاولة بعد تشغيل مارسا.";
const QUERY_TOO_LARGE: &str = "Too many bind sources in one request. Use fewer or shorter paths. | مصادر التركيب كثيرة في طلب واحد. استخدم مسارات أقل أو أقصر.";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Allow,
    Deny(String),
}

impl Decision {
    fn deny(message: &str) -> Self {
        Self::Deny(message.to_owned())
    }
}

#[derive(Deserialize)]
struct Request<'a> {
    #[serde(rename = "RequestMethod")]
    method: &'a str,
    #[serde(rename = "RequestUri")]
    uri: &'a str,
}

#[derive(Deserialize)]
struct CreateRequest<'a> {
    #[serde(rename = "RequestBody")]
    body: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
enum Operation {
    Other,
    Create,
    Reserved,
}

pub(crate) async fn authorize(callback: &[u8], policy: &Policy) -> Decision {
    let operation = serde_json::from_slice(callback)
        .ok()
        .and_then(|request| operation(&request));
    match operation {
        Some(Operation::Other) => Decision::Allow,
        Some(Operation::Reserved) => Decision::deny(RESERVED_PLUGIN),
        Some(Operation::Create) => {
            let Some(paths) = bind_sources(callback) else {
                return Decision::deny(INVALID_CREATE);
            };
            if paths.len() > MAX_BIND_AUTHORIZATION_SOURCES {
                return Decision::deny(QUERY_TOO_LARGE);
            }
            if paths.is_empty() {
                return Decision::Allow;
            }
            policy.authorize(paths).await
        }
        None => Decision::deny(INVALID_CALLBACK),
    }
}

fn operation(request: &Request<'_>) -> Option<Operation> {
    if request.method != "POST" && request.method != "DELETE" {
        return Some(Operation::Other);
    }
    let uri = request.uri.parse::<Uri>().ok()?;
    let decoded = percent_decode_str(uri.path()).decode_utf8().ok()?;
    // Gorilla's default path cleaner redirects these paths before routing;
    // normalize them here as well, then use its entire version token grammar.
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let parts = if parts.first().is_some_and(|part| {
        part.strip_prefix('v').is_some_and(|version| {
            !version.is_empty()
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
    }) {
        &parts[1..]
    } else {
        &parts[..]
    };
    if request.method == "POST" && parts == ["containers", "create"] {
        return Some(Operation::Create);
    }
    let reserved = match parts {
        ["plugins", "create" | "pull"] => uri.query().is_some_and(reserved_query),
        ["plugins", name, ..] => reserved_name(name),
        _ => false,
    };
    Some(if reserved {
        Operation::Reserved
    } else {
        Operation::Other
    })
}

fn reserved_query(query: &str) -> bool {
    query.split('&').any(|field| {
        field.split_once('=').is_some_and(|(key, value)| {
            let key = percent_decode_str(key).decode_utf8_lossy();
            let value = percent_decode_str(value).decode_utf8_lossy();
            matches!(key.as_ref(), "name" | "remote") && reserved_name(&value)
        })
    })
}

fn reserved_name(name: &str) -> bool {
    name == PLUGIN
        || name
            .split_once([':', '@'])
            .is_some_and(|(name, _)| name == PLUGIN)
}

fn bind_sources(callback: &[u8]) -> Option<Vec<String>> {
    let request: CreateRequest<'_> = serde_json::from_slice(callback).ok()?;
    if request.body.len() > MAX_CREATE_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let body = STANDARD.decode(request.body).ok()?;
    sources(&body)
        .ok()?
        .into_iter()
        .map(|source| source.into_os_string().into_string().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_versioned_and_decoded_create_paths_only_for_post() {
        let inspect = |method, uri| operation(&Request { method, uri });
        for uri in [
            "/containers/create?name=example",
            "/v1.53/containers/create",
            "/v1.53.0/containers/%63reate",
            "/v1.53/containers%2Fcreate",
            "http://docker/v1.53//containers/create",
        ] {
            assert_eq!(inspect("POST", uri), Some(Operation::Create), "{uri}");
            assert_eq!(inspect("GET", uri), Some(Operation::Other), "{uri}");
        }
        for uri in ["/build", "/grpc", "/containers/a/attach", "/images/load"] {
            assert_eq!(inspect("POST", uri), Some(Operation::Other), "{uri}");
        }
    }

    #[test]
    fn protects_reserved_name_and_keeps_normal_plugin_routes() {
        let inspect = |method, uri| operation(&Request { method, uri });
        for uri in [
            "/plugins/MarsaBindAuthorization/disable",
            "/v1.53/plugins/%4DarsaBindAuthorization%3Alatest/upgrade",
            "/plugins/create?name=MarsaBindAuthorization",
            "/plugins/pull?remote=MarsaBindAuthorization%3Alatest",
        ] {
            assert_eq!(inspect("POST", uri), Some(Operation::Reserved), "{uri}");
        }
        assert_eq!(
            inspect("DELETE", "/plugins/MarsaBindAuthorization"),
            Some(Operation::Reserved)
        );
        assert_eq!(
            inspect("POST", "/plugins/vendor/marsabindauthorization/disable"),
            Some(Operation::Other)
        );
    }

    #[tokio::test]
    async fn denies_uncheckable_creates_and_allows_unrelated_calls_offline() {
        let policy = Policy::default();
        let callback = |request_body: &str| {
            serde_json::to_vec(&serde_json::json!({
                "RequestMethod": "POST",
                "RequestUri": "/v1.53/containers/create",
                "RequestBody": request_body
            }))
            .unwrap()
        };
        assert_eq!(
            authorize(&callback("not base64"), &policy).await,
            Decision::deny(INVALID_CREATE)
        );
        assert_eq!(
            authorize(&callback(&STANDARD.encode(b"[not JSON")), &policy).await,
            Decision::deny(INVALID_CREATE)
        );
        assert_eq!(
            authorize(
                &callback(&STANDARD.encode(br#"{"HostConfig":{"Binds":["/host:/guest"]}}"#)),
                &policy
            )
            .await,
            Decision::deny(POLICY_UNAVAILABLE)
        );
        assert_eq!(
            authorize(
                &callback(&STANDARD.encode(br#"{"HostConfig":{"Binds":["named:/guest"]}}"#)),
                &policy
            )
            .await,
            Decision::Allow
        );
        assert_eq!(
            authorize(
                br#"{"RequestMethod":"POST","RequestUri":"/build","RequestBody":"ignored"}"#,
                &policy
            )
            .await,
            Decision::Allow
        );
    }
}
