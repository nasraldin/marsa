//! Keep workload credentials in a Keychain namespace scoped to one Compose project.

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use nix::unistd::geteuid;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::registry::helper::{self, Credential};
use crate::{compose::valid_project_name, docker_socket_path, platform};

const KEY_PREFIX: &str = "https://deploy-secrets.marsa.invalid/v1/";
const MAX_VALUE: usize = 2048;
const ENTRY_USER: &str = "marsa-compose";

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Revision {
    generation: u64,
    id: String,
}

impl Revision {
    fn key(&self, prefix: &str) -> String {
        format!("{prefix}{}-{}", self.generation, self.id)
    }

    fn parse(suffix: &str) -> Option<Self> {
        let (generation, id) = suffix.split_once('-')?;
        if id.len() != 36
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return None;
        }
        Some(Self {
            generation: generation.parse().ok()?,
            id: id.to_owned(),
        })
    }
}

/// Errors from the owner-private deploy secret provider.
#[derive(Debug, Error)]
pub enum DeploySecretError {
    /// The project or secret reference has an unsupported shape.
    #[error("invalid Compose project or secret name")]
    Reference,
    /// Compose skips empty values. Oversized values and binary data are unsupported.
    #[error("deploy secret must be bounded, non-empty text")]
    Value,
    /// The signed helper, owner state or Keychain could not be accessed.
    #[error("the deploy secret Keychain is unavailable")]
    Unavailable,
    /// The selected credential does not exist.
    #[error("no saved deploy secret matches the selected project")]
    Missing,
    /// The replacement saved successfully and a previous revision needs cleanup.
    #[error("the deploy secret was saved but its old Keychain revision remains")]
    Cleanup,
}

/// The macOS Keychain provider for a selected Compose file and project.
pub struct DeploySecretStore {
    helper: PathBuf,
    directory: PathBuf,
    file: PathBuf,
    project: String,
}

impl DeploySecretStore {
    /// Resolve an owner-selected local YAML path and discover the signed provider.
    pub async fn for_project(file: &Path, project: &str) -> Result<Self, DeploySecretError> {
        if geteuid().is_root() || !file.is_absolute() || !valid_project_name(project) {
            return Err(DeploySecretError::Reference);
        }
        let file = tokio::fs::canonicalize(file)
            .await
            .map_err(|_| DeploySecretError::Reference)?;
        if !file
            .extension()
            .and_then(|suffix| suffix.to_str())
            .is_some_and(|suffix| matches!(suffix, "yaml" | "yml"))
            || !tokio::fs::metadata(&file)
                .await
                .is_ok_and(|metadata| metadata.is_file())
        {
            return Err(DeploySecretError::Reference);
        }
        Self::for_canonical_project(&file, project).await
    }

    pub(crate) async fn for_canonical_project(
        file: &Path,
        project: &str,
    ) -> Result<Self, DeploySecretError> {
        if geteuid().is_root() || !file.is_absolute() || !valid_project_name(project) {
            return Err(DeploySecretError::Reference);
        }
        let helper = platform::bundled_registry_helper()
            .await
            .map_err(|_| DeploySecretError::Unavailable)?;
        let socket = docker_socket_path().map_err(|_| DeploySecretError::Unavailable)?;
        let directory = socket
            .parent()
            .and_then(Path::parent)
            .ok_or(DeploySecretError::Unavailable)?;
        Ok(Self {
            helper,
            directory: directory.to_owned(),
            file: file.to_owned(),
            project: project.to_owned(),
        })
    }

    /// Write a fresh revision before removing any earlier credential.
    pub async fn save(&self, name: &str, value: String) -> Result<(), DeploySecretError> {
        let prefix = self.prefix(name)?;
        if !valid_value(&value) {
            return Err(DeploySecretError::Value);
        }
        let _lock = self.lock().await?;
        let previous = self.revisions(&prefix).await?;
        let revision = Revision {
            generation: previous
                .last()
                .map_or(0, |entry| entry.generation)
                .checked_add(1)
                .ok_or(DeploySecretError::Unavailable)?,
            id: crate::request_id().map_err(|_| DeploySecretError::Unavailable)?,
        };
        helper::store(
            &self.helper,
            &Credential {
                server_url: revision.key(&prefix),
                username: ENTRY_USER.to_owned(),
                secret: value,
            },
        )
        .await
        .map_err(|_| DeploySecretError::Unavailable)?;
        for old in previous {
            helper::erase(&self.helper, &old.key(&prefix))
                .await
                .map_err(|_| DeploySecretError::Cleanup)?;
        }
        Ok(())
    }

    /// Remove the credential for this exact project and secret name.
    pub async fn remove(&self, name: &str) -> Result<(), DeploySecretError> {
        let prefix = self.prefix(name)?;
        let _lock = self.lock().await?;
        let revisions = self.revisions(&prefix).await?;
        if revisions.is_empty() {
            return Err(DeploySecretError::Missing);
        }
        for revision in revisions {
            helper::erase(&self.helper, &revision.key(&prefix))
                .await
                .map_err(|_| DeploySecretError::Unavailable)?;
        }
        Ok(())
    }

    pub(crate) async fn resolve_all(
        &self,
        names: &[String],
    ) -> Result<Vec<(String, String)>, DeploySecretError> {
        let _lock = self.lock().await?;
        let mut secrets = Vec::with_capacity(names.len());
        for name in names {
            let prefix = self.prefix(name)?;
            let revision = self
                .revisions(&prefix)
                .await?
                .pop()
                .ok_or(DeploySecretError::Missing)?;
            let value = helper::get(&self.helper, &revision.key(&prefix))
                .await
                .map_err(|_| DeploySecretError::Unavailable)?
                .ok_or(DeploySecretError::Missing)?;
            if value.username != ENTRY_USER || !valid_value(&value.secret) {
                return Err(DeploySecretError::Unavailable);
            }
            secrets.push((revision.id, value.secret));
        }
        Ok(secrets)
    }

    fn prefix(&self, name: &str) -> Result<String, DeploySecretError> {
        if !valid_name(name) {
            return Err(DeploySecretError::Reference);
        }
        Ok(key_prefix(&self.file, &self.project, name))
    }

    async fn revisions(&self, prefix: &str) -> Result<Vec<Revision>, DeploySecretError> {
        let mut revisions: Vec<_> = helper::list(&self.helper)
            .await
            .map_err(|_| DeploySecretError::Unavailable)?
            .into_iter()
            .filter(|(_, account)| account == ENTRY_USER)
            .filter_map(|(key, _)| Revision::parse(key.strip_prefix(prefix)?))
            .collect();
        revisions.sort_unstable();
        Ok(revisions)
    }

    async fn lock(&self) -> Result<File, DeploySecretError> {
        let directory = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            platform::lock_docker_state(&directory, "desktop-deploy-secrets.lock")
        })
        .await
        .map_err(|_| DeploySecretError::Unavailable)?
        .map_err(|_| DeploySecretError::Unavailable)
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE
        && !value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

fn key_prefix(file: &Path, project: &str, name: &str) -> String {
    let mut key = Sha256::new();
    key.update(file.as_os_str().as_encoded_bytes());
    key.update([0]);
    key.update(project.as_bytes());
    key.update([0]);
    key.update(name.as_bytes());
    format!("{KEY_PREFIX}{}/", URL_SAFE_NO_PAD.encode(key.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_scope_includes_path_project_and_secret_name() {
        let name = "database-password";
        let compose = Path::new("/private/project/compose.yaml");
        let key = key_prefix(compose, "project", name);
        assert_ne!(key, key_prefix(compose, "other", name));
        assert_ne!(key, key_prefix(compose, "project", "api-key"));
        assert_ne!(
            key,
            key_prefix(Path::new("/other/compose.yaml"), "project", name)
        );
        assert!(!key.contains("database-password"));
        assert!(key.starts_with(KEY_PREFIX));
    }

    #[test]
    fn invalid_names_and_values_fail_before_keychain_access() {
        for name in [
            ".",
            "../key",
            "secret file",
            "secret/file",
            "$injection",
            "",
            "a\n",
        ] {
            assert!(!valid_name(name), "{name:?}");
        }
        assert!(valid_name("test_secret-1"));
        assert!(!valid_value(""));
        assert!(!valid_value("a\0b"));
        assert!(valid_value("value with spaces and\nnewlines"));
    }
    #[tokio::test]
    #[ignore = "requires the pinned macOS credential helper and an unlocked Keychain"]
    async fn saved_secret_rotates_in_an_isolated_keychain_namespace() {
        let helper =
            std::env::var_os("MARSA_TEST_KEYCHAIN_HELPER").expect("set MARSA_TEST_KEYCHAIN_HELPER");
        let scratch = tempfile::tempdir().unwrap();
        let store = DeploySecretStore {
            helper: helper.into(),
            directory: scratch.path().to_owned(),
            file: scratch.path().join("compose.yaml"),
            project: "keychain-fixture".into(),
        };
        let value = crate::request_id().unwrap();
        let names = vec!["credential".to_owned()];
        let run = async {
            store.save(&names[0], value.clone()).await?;
            let first = store.resolve_all(&names).await?;
            store.save(&names[0], value.clone()).await?;
            let second = store.resolve_all(&names).await?;
            // Saving the same value still changes the public revision.
            Ok::<_, DeploySecretError>(
                first[0].0 != second[0].0
                    && first[0].1 == value
                    && second[0].1 == value
                    && first[0].0 != value
                    && second[0].0 != value,
            )
        }
        .await;
        let cleanup = store.remove(&names[0]).await;
        cleanup.expect("could not remove the synthetic fixture credential");
        assert!(run.expect("synthetic Keychain provider test failed"));
        assert!(matches!(
            store.resolve_all(&names).await,
            Err(DeploySecretError::Missing)
        ));
    }
}
