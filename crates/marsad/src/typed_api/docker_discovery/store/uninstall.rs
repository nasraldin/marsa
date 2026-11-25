//! Authorize removal of the one owned metadata entry without changing other Docker settings.

use std::{fs::Metadata as FileMetadata, path::PathBuf};

use super::*;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// A failed preflight or removal of the registered Marsa Docker context.
#[derive(Debug, Error)]
pub enum ContextRemovalError {
    /// Docker is configured to keep using the exact context.
    #[error("the Marsa Docker context is active")]
    Active,
    /// A Docker config entry was unsafe to read or disclose.
    #[error("the Docker configuration is unsafe for context removal")]
    Unsafe,
    /// A filesystem operation failed.
    #[error("could not read or remove the Docker context")]
    Io(#[from] io::Error),
}

/// A validated, optional owner context removal.
pub struct ContextRemoval {
    config: PathBuf,
    expected: Metadata,
    uid: u32,
}

/// Preflight the matching user context and leave foreign registrations in place.
///
/// The selection and context identity are checked again immediately before removal.
pub fn prepare_context_removal(
    config: &Path,
    socket: &Path,
    uid: u32,
) -> Result<ContextRemoval, ContextRemovalError> {
    let socket = socket.to_str().ok_or(ContextRemovalError::Unsafe)?;
    let removal = ContextRemoval {
        config: config.to_owned(),
        expected: Metadata {
            name: NAME.to_owned(),
            metadata: Description {
                description: DESCRIPTION.to_owned(),
            },
            endpoints: DockerOnly {
                docker: Endpoint {
                    host: format!("unix://{socket}"),
                    skip_tls_verify: false,
                },
            },
        },
        uid,
    };
    removal.open()?;
    Ok(removal)
}

impl ContextRemoval {
    /// Remove only the verified registration. An absent context is already removed.
    pub fn remove(self) -> Result<(), ContextRemovalError> {
        let Some(OwnedContext {
            meta,
            record_dir,
            record,
        }) = self.open()?
        else {
            return Ok(());
        };
        check_identity(&meta, STORE_ID, &record_dir.metadata()?)?;
        check_identity(&record_dir, RECORD, &record.metadata()?)?;
        unlinkat(&record_dir, RECORD, UnlinkatFlags::NoRemoveDir).map_err(io::Error::from)?;
        unlinkat(&meta, STORE_ID, UnlinkatFlags::RemoveDir).map_err(io::Error::from)?;
        meta.sync_all()?;
        Ok(())
    }

    fn open(&self) -> Result<Option<OwnedContext>, ContextRemovalError> {
        validate_directory(&self.config).map_err(|_| ContextRemovalError::Unsafe)?;
        let parent = self.config.parent().ok_or(ContextRemovalError::Unsafe)?;
        let parent = match parent.canonicalize() {
            Ok(parent) => parent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORIES.bits())
            .open(parent)?;
        check_directory(&parent, self.uid).map_err(translate)?;
        let config_name = self.config.file_name().ok_or(ContextRemovalError::Unsafe)?;
        let Some(config) = existing_directory(&parent, config_name, self.uid)? else {
            return Ok(None);
        };
        let Some(contexts) = existing_directory(&config, OsStr::new("contexts"), self.uid)? else {
            return Ok(None);
        };
        let Some(meta) = existing_directory(&contexts, OsStr::new("meta"), self.uid)? else {
            return Ok(None);
        };
        let Some(record_dir) = existing_directory(&meta, OsStr::new(STORE_ID), self.uid)? else {
            return Ok(None);
        };
        let Some(record) = existing_file(&record_dir, RECORD, self.uid)? else {
            return Ok(None);
        };
        let raw = read_record(record.try_clone()?)?;
        if serde_json::from_slice::<Metadata>(&raw).ok().as_ref() != Some(&self.expected) {
            return Ok(None);
        }
        match reject_tls(&contexts, self.uid) {
            Ok(()) => (),
            Err(Error::Conflict) => return Ok(None),
            Err(error) => return Err(translate(error)),
        }
        for result in rustix::fs::Dir::read_from(&record_dir).map_err(io::Error::from)? {
            let entry = result.map_err(io::Error::from)?;
            if !matches!(entry.file_name().to_bytes(), b"." | b".." | b"meta.json") {
                return Err(ContextRemovalError::Unsafe);
            }
        }
        if active(&config, self.uid)? {
            return Err(ContextRemovalError::Active);
        }
        Ok(Some(OwnedContext {
            meta,
            record_dir,
            record,
        }))
    }
}

struct OwnedContext {
    meta: File,
    record_dir: File,
    record: File,
}

fn existing_directory(
    parent: &File,
    name: &OsStr,
    uid: u32,
) -> Result<Option<File>, ContextRemovalError> {
    match open_directory(parent, name, uid) {
        Ok(directory) => Ok(Some(directory)),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(translate(error)),
    }
}

fn existing_file(parent: &File, name: &str, uid: u32) -> Result<Option<File>, ContextRemovalError> {
    let file = match openat(parent, name, READ_FILE | OFlag::O_CLOEXEC, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(translate(classify_path(error.into()))),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o6022 != 0
    {
        return Err(ContextRemovalError::Unsafe);
    }
    Ok(Some(file))
}

fn active(config: &File, uid: u32) -> Result<bool, ContextRemovalError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Selection {
        current_context: Option<String>,
    }

    let Some(config) = existing_file(config, "config.json", uid)? else {
        return Ok(false);
    };
    let mut buffer = Vec::new();
    config.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut buffer)?;
    if buffer.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ContextRemovalError::Unsafe);
    }
    serde_json::from_slice::<Selection>(&buffer)
        .map(|selection| selection.current_context.as_deref() == Some(NAME))
        .map_err(|_| ContextRemovalError::Unsafe)
}

fn check_identity(
    parent: &File,
    member: &str,
    expected: &FileMetadata,
) -> Result<(), ContextRemovalError> {
    let actual = fstatat(parent, member, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if actual.st_dev as u64 != expected.dev() || actual.st_ino != expected.ino() {
        return Err(ContextRemovalError::Unsafe);
    }
    Ok(())
}

fn translate(error: Error) -> ContextRemovalError {
    match error {
        Error::Io(error) => error.into(),
        _ => ContextRemovalError::Unsafe,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const OWNER_SOCKET: &str = "/Users/fixture/.marsa/run/docker.sock";

    fn install_fixture(host: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join(".docker");
        let staged = directory.path().join("stage");
        let metadata = Path::new("contexts").join("meta").join(STORE_ID);
        let source = staged.join(&metadata);
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join(RECORD),
            serde_json::to_vec(&serde_json::json!({
                "Name": NAME,
                "Metadata": {"Description": DESCRIPTION},
                "Endpoints": {"docker": {"Host": host, "SkipTLSVerify": false}}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(install(&staged, &config, nix::unistd::geteuid().as_raw(), host).unwrap());
        let installed = config.join(metadata);
        (directory, config, installed)
    }

    fn prepare(config: &Path) -> Result<ContextRemoval, ContextRemovalError> {
        prepare_context_removal(
            config,
            Path::new(OWNER_SOCKET),
            nix::unistd::geteuid().as_raw(),
        )
    }

    #[test]
    fn removes_only_the_owned_context_and_preserves_docker_configuration() {
        let (_owner, config, context) = install_fixture(&format!("unix://{OWNER_SOCKET}"));
        let selected = r#"{"currentContext":"another","credsStore":"osxkeychain"}"#;
        let other = context.parent().unwrap().join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join(RECORD), "other context").unwrap();
        fs::write(config.join("config.json"), selected).unwrap();

        prepare(&config).unwrap().remove().unwrap();
        prepare(&config).unwrap().remove().unwrap();

        assert!(!context.exists());
        assert_eq!(
            fs::read_to_string(other.join(RECORD)).unwrap(),
            "other context"
        );
        assert_eq!(
            fs::read_to_string(config.join("config.json")).unwrap(),
            selected
        );
    }

    #[test]
    fn refuses_a_live_selection_even_if_it_changed_after_preflight() {
        let (_owner, config, context) = install_fixture(&format!("unix://{OWNER_SOCKET}"));
        let consent = prepare(&config).unwrap();
        fs::write(config.join("config.json"), r#"{"currentContext":"marsa"}"#).unwrap();
        assert!(matches!(consent.remove(), Err(ContextRemovalError::Active)));
        assert!(matches!(prepare(&config), Err(ContextRemovalError::Active)));
        assert!(context.join(RECORD).is_file());
    }

    #[test]
    fn preserves_foreign_names_and_tls_registrations() {
        let (_owner, config, context) = install_fixture("ssh://other.example.invalid");
        prepare(&config).unwrap().remove().unwrap();
        assert!(context.join(RECORD).is_file());

        let (_owner, config, context) = install_fixture(&format!("unix://{OWNER_SOCKET}"));
        let tls = config.join("contexts/tls").join(STORE_ID);
        fs::create_dir_all(&tls).unwrap();
        fs::write(tls.join("ca.pem"), "fixture TLS data").unwrap();
        prepare(&config).unwrap().remove().unwrap();
        assert!(context.join(RECORD).is_file());
        assert!(tls.join("ca.pem").is_file());
    }

    #[test]
    fn refuses_additional_unrecognized_data_without_removing_registration() {
        let (_owner, config, context) = install_fixture(&format!("unix://{OWNER_SOCKET}"));
        let unrelated = context.join("unrecognized");
        fs::write(&unrelated, "preserve").unwrap();

        assert!(matches!(prepare(&config), Err(ContextRemovalError::Unsafe)));
        assert!(context.join(RECORD).is_file());
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "preserve");
    }
}
