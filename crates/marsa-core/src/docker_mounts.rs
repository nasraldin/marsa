//! Identify mounts that Docker would interpret as host bind sources.

use std::{fmt, path::PathBuf};

use serde::{
    Deserialize, Deserializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};
use thiserror::Error;

/// Maximum buffered body size for an inspected ContainerCreate request.
pub const MAX_CREATE_BYTES: usize = 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;

/// A malformed create request cannot be evaluated against host share policy.
#[derive(Debug, Error)]
pub enum MountRequestError {
    /// The request exceeds the supported inspection size.
    #[error("the container create request exceeds the inspection limit")]
    TooLarge,
    /// The encoded settings are ambiguous or malformed.
    #[error("invalid or ambiguous Docker mount configuration")]
    InvalidJson(#[source] serde_json::Error),
    /// The engine would receive an invalid host path.
    #[error("a Docker bind source must be an absolute host path")]
    InvalidSource,
}

/// Return absolute host bind sources from the ContainerCreate body.
///
/// Named volumes and container-only mounts do not have a host source.
/// The caller must check every returned source against the applied allowlist.
pub fn sources(body: &[u8]) -> Result<Vec<PathBuf>, MountRequestError> {
    if body.len() > MAX_CREATE_BYTES {
        return Err(MountRequestError::TooLarge);
    }
    let create: Create = serde_json::from_slice(body).map_err(MountRequestError::InvalidJson)?;
    let Some(host) = create.0 else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for bind in host.binds {
        // Moby uses a colon to separate an explicit volume/bind source from
        // its target. An entry without that separator has only a target.
        if let Some((source, _)) = bind.split_once(':')
            && source.starts_with('/')
        {
            paths.push(host_path(source)?);
        }
    }
    for mount in host.mounts {
        if mount.kind.as_deref() == Some("bind") {
            let source = mount.source.ok_or(MountRequestError::InvalidSource)?;
            paths.push(host_path(&source)?);
        }
    }
    Ok(paths)
}

fn host_path(source: &str) -> Result<PathBuf, MountRequestError> {
    if !source.starts_with('/')
        || source.len() > MAX_PATH_BYTES
        || source.chars().any(char::is_control)
    {
        return Err(MountRequestError::InvalidSource);
    }
    Ok(PathBuf::from(source))
}

struct Create(Option<HostConfig>);

struct HostConfig {
    binds: Vec<String>,
    mounts: Vec<Mount>,
}

struct Mount {
    kind: Option<String>,
    source: Option<String>,
}

impl<'de> Deserialize<'de> for Create {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(RecordVisitor(std::marker::PhantomData))
    }
}

impl Record for Create {
    fn read<'de, A: MapAccess<'de>>(mut fields: A) -> Result<Self, A::Error> {
        let mut host = None;
        while let Some(name) = field(&mut fields)? {
            if name.eq_ignore_ascii_case("HostConfig") {
                read_once(&mut fields, &mut host)?;
            } else {
                fields.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Self(host.flatten()))
    }
}

impl<'de> Deserialize<'de> for HostConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(RecordVisitor(std::marker::PhantomData))
    }
}

impl Record for HostConfig {
    fn read<'de, A: MapAccess<'de>>(mut fields: A) -> Result<Self, A::Error> {
        let (mut binds, mut mounts) = (None, None);
        while let Some(name) = field(&mut fields)? {
            if name.eq_ignore_ascii_case("Binds") {
                read_once(&mut fields, &mut binds)?;
            } else if name.eq_ignore_ascii_case("Mounts") {
                read_once(&mut fields, &mut mounts)?;
            } else {
                fields.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Self {
            binds: binds.flatten().unwrap_or_default(),
            mounts: mounts.flatten().unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for Mount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(RecordVisitor(std::marker::PhantomData))
    }
}

impl Record for Mount {
    fn read<'de, A: MapAccess<'de>>(mut fields: A) -> Result<Self, A::Error> {
        let (mut kind, mut source) = (None, None);
        while let Some(name) = field(&mut fields)? {
            if name.eq_ignore_ascii_case("Type") {
                read_once(&mut fields, &mut kind)?;
            } else if name.eq_ignore_ascii_case("Source") {
                read_once(&mut fields, &mut source)?;
            } else {
                fields.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Self {
            kind: kind.flatten(),
            source: source.flatten(),
        })
    }
}

fn field<'de, A: MapAccess<'de>>(fields: &mut A) -> Result<Option<String>, A::Error> {
    let name: Option<String> = fields.next_key()?;
    // Go's field matching also folds some Unicode glyphs to ASCII. Reject
    // non-ASCII schema keys here instead of interpreting them differently.
    // Object keys inside ignored fields (such as Labels) are unaffected.
    if name.as_ref().is_some_and(|name| !name.is_ascii()) {
        return Err(de::Error::custom("non-ASCII Docker schema key"));
    }
    Ok(name)
}

fn read_once<'de, A, T>(fields: &mut A, slot: &mut Option<Option<T>>) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::custom("duplicate Docker mount field"));
    }
    *slot = Some(fields.next_value()?);
    Ok(())
}

trait Record: Sized {
    fn read<'de, A: MapAccess<'de>>(fields: A) -> Result<Self, A::Error>;
}

struct RecordVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T: Record> Visitor<'de> for RecordVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a Docker schema object")
    }

    fn visit_map<A: MapAccess<'de>>(self, fields: A) -> Result<T, A::Error> {
        T::read(fields)
    }
}
