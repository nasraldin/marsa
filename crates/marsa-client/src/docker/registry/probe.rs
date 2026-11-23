//! Verify credentials at their selected TLS authority without delegating to an engine.

use hyper::Uri;
use serde::Deserialize;

use super::{RegistryAddress, RegistryCredentials, RegistryError};

use crate::platform::registry_https;

const HUB_TOKEN_ENDPOINT: &str = "https://auth.docker.io/token";
const HUB_TOKEN_SERVICE: &str = "registry.docker.io";

pub(super) async fn check(
    address: &RegistryAddress,
    credential: RegistryCredentials,
) -> Result<(), RegistryError> {
    if address.is_docker_hub() {
        return request_token(HUB_TOKEN_ENDPOINT, HUB_TOKEN_SERVICE, &credential).await;
    }
    let url = registry_url(address);
    let anonymous = request(&url, None, None).await?;
    if basic_challenge(&anonymous) {
        let verified = request(&url, Some(&credential), None).await?;
        if response(&verified).is_some_and(|value| value.status == "200") {
            return Ok(());
        }
        return Err(RegistryError::Connection);
    }

    let (realm, service) = bearer_challenge(&anonymous, address).ok_or(RegistryError::Challenge)?;
    request_token(realm, service, &credential).await
}

async fn request(
    url: &str,
    credential: Option<&RegistryCredentials>,
    service: Option<&str>,
) -> Result<String, RegistryError> {
    registry_https::get(
        url,
        credential.map(|value| (value.username.as_str(), value.secret.as_str())),
        service,
    )
    .await
    .map_err(|_| RegistryError::Connection)
}

pub(super) fn registry_url(address: &RegistryAddress) -> String {
    let host = if address.is_docker_hub() {
        "registry-1.docker.io"
    } else {
        address.as_str()
    };
    format!("https://{host}/v2/")
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: String,
    #[serde(default)]
    access_token: String,
}

async fn request_token(
    realm: &str,
    service: &str,
    credential: &RegistryCredentials,
) -> Result<(), RegistryError> {
    let result = request(realm, Some(credential), Some(service)).await?;
    let reply = response(&result).ok_or(RegistryError::Connection)?;
    if reply.status != "200" {
        return Err(RegistryError::Connection);
    }
    let token: TokenResponse =
        serde_json::from_str(reply.body).map_err(|_| RegistryError::Connection)?;
    if token.token.is_empty() && token.access_token.is_empty() {
        return Err(RegistryError::Connection);
    }
    Ok(())
}

struct HttpResponse<'a> {
    status: &'a str,
    headers: &'a str,
    body: &'a str,
}

fn response(source: &str) -> Option<HttpResponse<'_>> {
    let (headers, body) = source.split_once("\r\n\r\n")?;
    let (status, fields) = headers.split_once("\r\n").unwrap_or((headers, ""));
    let status = status
        .strip_prefix("HTTP/1.1 ")?
        .split_whitespace()
        .next()?;
    Some(HttpResponse {
        status,
        headers: fields,
        body,
    })
}

fn challenges(source: &str) -> impl Iterator<Item = &str> {
    response(source)
        .filter(|reply| reply.status == "401")
        .into_iter()
        .flat_map(|reply| reply.headers.lines())
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("www-authenticate")
                .then_some(value.trim())
        })
}

pub(super) fn basic_challenge(source: &str) -> bool {
    challenges(source).any(|value| {
        value
            .split_once(' ')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("basic"))
    })
}

fn bearer_challenge<'a>(source: &'a str, address: &RegistryAddress) -> Option<(&'a str, &'a str)> {
    let mut matches = challenges(source).filter_map(|value| {
        let (name, fields) = value.split_once(' ')?;
        name.eq_ignore_ascii_case("bearer").then_some(fields)
    });
    let fields = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let mut realm = None;
    let mut service = None;
    for field in fields.split(',') {
        let (key, value) = field.trim().split_once('=')?;
        let value = value.strip_prefix('"')?.strip_suffix('"')?;
        if value.contains(['"', '\\']) || value.chars().any(char::is_control) {
            return None;
        }
        match key {
            "realm" if realm.is_none() => realm = Some(value),
            "service" if service.is_none() => service = Some(value),
            "scope" => (),
            _ => return None,
        }
    }
    let realm = realm?;
    let service = service?;
    let url = realm.parse::<Uri>().ok()?;
    if realm.len() > 2048
        || realm.contains('#')
        || service.is_empty()
        || service.len() > 512
        || url.scheme_str() != Some("https")
        || url.authority().map(|id| id.as_str()) != Some(address.as_str())
        || url.query().is_some()
    {
        return None;
    }
    Some((realm, service))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_requires_exact_https_authority_without_query_or_redirect() {
        let address = RegistryAddress::parse("registry.example:5443").unwrap();
        let challenge = |realm| {
            format!(
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"{realm}\",service=\"distribution\"\r\n\r\n"
            )
        };
        let realm = "https://registry.example:5443/token";
        assert_eq!(
            bearer_challenge(&challenge(realm), &address),
            Some((realm, "distribution"))
        );
        for realm in [
            "http://registry.example:5443/token",
            "https://registry.example/token",
            "https://registry.example:5443@other.example/token",
            "https://other.example:5443/token",
            "https://child.registry.example:5443/token",
            "https://registry.example:5443/token?service=other",
        ] {
            assert!(
                bearer_challenge(&challenge(realm), &address).is_none(),
                "{realm}"
            );
        }
    }
}
