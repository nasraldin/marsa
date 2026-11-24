use std::{convert::TryFrom, hash::Hash, ops::Deref, str::FromStr};

use iri_string::types::UriReferenceString;
use serde::{Deserialize, Serialize, de::Error};

/// Newtype struct around `UriReferenceString` with serialization implementations that use `as_str()` and 'from_str()' respectively.
#[derive(Debug, Clone)]
pub struct Uri(UriReferenceString);

impl Serialize for Uri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Uri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        UriReferenceString::try_from(string)
            .map(Uri)
            .map_err(Error::custom)
    }
}

impl Ord for Uri {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Uri {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for Uri {
    type Err = iri_string::validate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl Deref for Uri {
    type Target = UriReferenceString;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq for Uri {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Uri {}

impl Hash for Uri {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_references_keep_original_spelling_and_optional_scheme() {
        for value in [
            "file:///a%20b",
            "HTTPS://example.org/A%2fb?keep=Raw#anchor",
            "untitled:Untitled-1",
            "vfs://[2001:db8::1]/a",
            "../relative%20file",
            "#fragment",
            "",
        ] {
            let uri = value.parse::<Uri>().unwrap();
            assert_eq!(uri.as_str(), value);
            let serialized = serde_json::to_string(&uri).unwrap();
            let round_tripped = serde_json::from_str::<Uri>(&serialized).unwrap();
            assert_eq!(round_tripped, uri);
        }

        assert_eq!(
            "http://example.org".parse::<Uri>().unwrap().scheme_str(),
            Some("http")
        );
        assert_eq!("../relative".parse::<Uri>().unwrap().scheme_str(), None);
        for invalid in [
            "file:///[x]",
            "http://[::",
            "bad path",
            "file:///%yz",
            "مجلد",
        ] {
            assert!(invalid.parse::<Uri>().is_err(), "invalid URI: {}", invalid);
        }
    }
}
