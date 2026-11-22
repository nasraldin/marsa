//! Enable an explicit credential-store opt-in without rewriting unrelated Docker fields.

use std::{
    fmt,
    fs::{File, Metadata, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, openat, renameat},
    sys::stat::Mode,
    unistd::{UnlinkatFlags, unlinkat},
};
use rustix::fs::{RenameFlags, renameat_with};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::Value;
use thiserror::Error;
use tonic::Status;

use super::store;

const CONFIG: &str = "config.json";
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const STORE_FIELD: &[u8] = br#""credsStore": "osxkeychain""#;
const FILE_FLAGS: OFlag = OFlag::O_NOFOLLOW
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC);

#[derive(Debug, Error)]
pub(super) enum Error {
    #[error("Docker's configuration path is not a safe user-owned location")]
    Unsafe,
    #[error("Docker's configuration is invalid or too large")]
    Malformed,
    #[error(
        "Docker has stored credentials in its configuration. Log out and reauthenticate after enabling Keychain."
    )]
    ExistingCredentials,
    #[error("Docker's configuration changed while it was being edited")]
    Changed,
    #[error("could not update Docker's configuration")]
    Io(#[from] io::Error),
}

pub(super) fn status(error: Error) -> Status {
    match error {
        Error::Changed => Status::aborted(error.to_string()),
        Error::Unsafe | Error::Malformed | Error::ExistingCredentials => {
            Status::failed_precondition(error.to_string())
        }
        Error::Io(error) => {
            tracing::warn!(kind = ?error.kind(), "Docker credential store configuration failed");
            Status::internal("could not update Docker's configuration")
        }
    }
}

pub(super) fn enable(location: &Path, uid: u32) -> Result<bool, Error> {
    marsa_core::paths::validate_directory(location).map_err(|_| Error::Unsafe)?;
    let parent = location.parent().ok_or(Error::Unsafe)?.canonicalize()?;
    let parent = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(parent)?;
    store::check_directory(&parent, uid).map_err(|_| Error::Unsafe)?;
    let directory =
        store::ensure_directory(&parent, location.file_name().ok_or(Error::Unsafe)?, uid)
            .map_err(|_| Error::Unsafe)?;
    let current = read(&directory, uid)?;
    let raw = current.as_ref().map_or(b"{}" as &[u8], |(_, bytes)| bytes);
    let Some(updated) = add_setting(raw)? else {
        return Ok(false);
    };
    publish(&directory, uid, current, &updated)?;
    Ok(true)
}

fn add_setting(raw: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let Configuration(settings) = serde_json::from_slice(raw).map_err(|_| Error::Malformed)?;
    if settings.keys().any(|key| {
        key.eq_ignore_ascii_case("credsStore") || key.eq_ignore_ascii_case("credHelpers")
    }) {
        return Ok(None);
    }
    let has_tokens = settings.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("auths")
            && !value.is_null()
            && value.as_object().is_none_or(|auths| {
                auths.values().any(|entry| {
                    entry.as_object().is_none_or(|fields| {
                        fields
                            .values()
                            .any(|value| !matches!(value.as_str(), Some("")))
                    })
                })
            })
    });
    if has_tokens {
        return Err(Error::ExistingCredentials);
    }

    // Preserve all original bytes, including unknown keys and formatting.
    let opening = raw
        .iter()
        .position(|byte| *byte == b'{')
        .ok_or(Error::Malformed)?
        + 1;
    let mut result = raw[..opening].to_vec();
    result.extend_from_slice(b"\n  ");
    result.extend_from_slice(STORE_FIELD);
    if !settings.is_empty() {
        result.push(b',');
    }
    result.extend_from_slice(&raw[opening..]);
    Ok(Some(result))
}

struct Configuration(serde_json::Map<String, Value>);

impl<'de> Deserialize<'de> for Configuration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueFields;

        impl<'de> Visitor<'de> for UniqueFields {
            type Value = Configuration;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Docker configuration object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut unique = serde_json::Map::new();
                while let Some(field) = map.next_key::<String>()? {
                    if !field.is_ascii() {
                        return Err(de::Error::custom("non-ASCII Docker configuration field"));
                    }
                    // Docker's Go decoder matches struct keys case-insensitively.
                    let field = field.to_ascii_lowercase();
                    if unique.contains_key(&field) {
                        return Err(de::Error::custom("duplicate Docker configuration field"));
                    }
                    unique.insert(field, map.next_value()?);
                }
                Ok(Configuration(unique))
            }
        }

        deserializer.deserialize_map(UniqueFields)
    }
}

fn read(directory: &File, uid: u32) -> Result<Option<(Metadata, Vec<u8>)>, Error> {
    let file = match openat(
        directory,
        CONFIG,
        FILE_FLAGS | OFlag::O_RDONLY,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(Errno::ENOENT) => return Ok(None),
        Err(Errno::ELOOP | Errno::ENOTDIR | Errno::EACCES) => return Err(Error::Unsafe),
        Err(error) => return Err(io::Error::from(error).into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o6022 != 0
    {
        return Err(Error::Unsafe);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(Error::Malformed);
    }
    Ok(Some((metadata, bytes)))
}

fn publish(
    directory: &File,
    uid: u32,
    current: Option<(Metadata, Vec<u8>)>,
    updated: &[u8],
) -> Result<(), Error> {
    let nonce = getrandom::u64().map_err(io::Error::other)?;
    let staging = format!(".marsa-keychain-{nonce:016x}");
    let mut file = File::from(
        openat(
            directory,
            staging.as_str(),
            FILE_FLAGS | OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .map_err(io::Error::from)?,
    );
    let result = (|| {
        file.set_permissions(Permissions::from_mode(0o600))?;
        file.write_all(updated)?;
        file.sync_all()?;
        match current {
            Some((original, bytes)) => {
                let Some((latest, latest_bytes)) = read(directory, uid)? else {
                    return Err(Error::Changed);
                };
                if identity(&latest) != identity(&original) || latest_bytes != bytes {
                    return Err(Error::Changed);
                }
                renameat(directory, staging.as_str(), directory, CONFIG)
                    .map_err(io::Error::from)?;
            }
            None => renameat_with(
                directory,
                staging.as_str(),
                directory,
                CONFIG,
                RenameFlags::NOREPLACE,
            )
            .map_err(|error| {
                if error == rustix::io::Errno::EXIST {
                    Error::Changed
                } else {
                    Error::Io(error.into())
                }
            })?,
        }
        if let Err(error) = directory.sync_all() {
            tracing::warn!(kind = ?error.kind(), "Docker configuration directory sync failed");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = unlinkat(directory, staging.as_str(), UnlinkatFlags::NoRemoveDir);
    }
    result
}

fn identity(metadata: &Metadata) -> (u64, u64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

#[cfg(test)]
mod tests;
