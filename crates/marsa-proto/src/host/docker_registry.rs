//! Validate a tagged image before constructing a passive Docker registry route.

use std::net::Ipv6Addr;

use serde::Deserialize;

/// Supported host API revision for anonymous, authorized registry digest requests.
pub const REGISTRY_DIGEST_API_MINOR: u32 = 17;

// The daemon may need a registry challenge, a token and a manifest in one lookup.
/// Maximum time budget for one passive managed registry request.
pub const REGISTRY_DIGEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

/// Build the Docker distribution route for a bounded named tag.
///
/// The accepted grammar excludes image IDs, digests, schemes, traversal, percent escapes
/// and query syntax. The daemon still resolves the reference using Docker's own parser.
pub fn registry_digest_path(reference: &str) -> Option<String> {
    let (name, tag) = reference.rsplit_once(':')?;
    if name.is_empty()
        || name.len() > 255
        || name == "sha256"
        || tag.is_empty()
        || tag.len() > 128
        || tag.starts_with(['.', '-'])
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return None;
    }

    let repository = if let Some((first, rest)) = name.split_once('/')
        && (first == "localhost"
            || first.contains(['.', ':'])
            || first.bytes().any(|byte| byte.is_ascii_uppercase()))
    {
        if !valid_registry(first) {
            return None;
        }
        rest
    } else {
        name
    };
    repository
        .split('/')
        .all(valid_component)
        .then(|| format!("/distribution/{reference}/json"))
}

fn valid_registry(registry: &str) -> bool {
    if let Some(ipv6) = registry.strip_prefix('[') {
        return ipv6.split_once(']').is_some_and(|(address, rest)| {
            address.parse::<Ipv6Addr>().is_ok()
                && (rest.is_empty() || rest.strip_prefix(':').is_some_and(valid_port))
        });
    }
    let (host, port) = registry
        .split_once(':')
        .map_or((registry, None), |(host, port)| (host, Some(port)));
    port.is_none_or(valid_port)
        && host.split('.').all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('-')
                && !segment.ends_with('-')
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

fn valid_component(mut component: &str) -> bool {
    loop {
        let name = component.trim_start_matches(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        });
        if name.len() == component.len() {
            return false;
        }
        let Some(separator) = name.chars().next() else {
            return true;
        };
        component = name.trim_start_matches(separator);
        let count = name.len() - component.len();
        if !matches!(separator, '.' | '_' | '-')
            || (separator == '.' && count != 1)
            || (separator == '_' && count > 2)
        {
            return false;
        }
    }
}

/// Docker's distribution response with the descriptor needed for update comparisons.
#[derive(Deserialize)]
pub struct DistributionInspect {
    #[serde(rename = "Descriptor")]
    descriptor: DistributionDescriptor,
}

#[derive(Deserialize)]
struct DistributionDescriptor {
    digest: String,
}

impl DistributionInspect {
    /// Return the descriptor's validated SHA-256 digest.
    pub fn into_digest(self) -> Option<String> {
        valid_registry_digest(&self.descriptor.digest).then_some(self.descriptor.digest)
    }
}

/// Check the digest shape received from a passive owner RPC or Docker API.
pub fn valid_registry_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_bounded_tagged_names() {
        for valid in [
            "alpine:latest",
            "org/image__name:Build_2.0",
            "registry.example:5000/team/dev--image:2026",
            "[2001:db8::1]:5000/team/app:latest",
        ] {
            assert_eq!(
                registry_digest_path(valid),
                Some(format!("/distribution/{valid}/json"))
            );
        }
        for invalid in [
            "alpine",
            "sha256:1234",
            "alpine:dev?auth=on",
            "https://example.test/a:latest",
            "example.test/../a:latest",
            "org//a:latest",
            "org/a:%00",
            "org/%2e%2e/a:latest",
            "org/a:_ok#bad",
            "org/dev___image:latest",
            "example.test:-1/a:latest",
            "example.test:65536/a:latest",
            "org/a:-leading-dash",
        ] {
            assert!(registry_digest_path(invalid).is_none(), "{invalid}");
        }
    }
}
