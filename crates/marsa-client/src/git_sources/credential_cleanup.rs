//! Retain exact obsolete Keychain keys until their deletion can be retried.

use std::{
    collections::{BTreeSet, HashSet},
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{
    GitSourceError,
    https_credentials::{erase_key, valid_key},
};
use crate::platform;

const JOURNAL: &str = "git-credential-cleanup.json";
const MAX_KEYS: usize = 256;
const MAX_BYTES: usize = 32 * 1024;
const MAX_ERASES: usize = 4;

pub(super) struct Journal {
    directory: PathBuf,
    keys: BTreeSet<String>,
}

impl Journal {
    /// A replacement can need two additional records before writing Keychain.
    pub(super) fn needs_capacity(&self) -> bool {
        self.keys.len() > MAX_KEYS - 2
    }

    /// Read the journal while the caller holds the shared repository mutation lock.
    pub(super) async fn load(directory: &Path) -> Result<Self, GitSourceError> {
        platform::check_user_docker_directory(directory)
            .await
            .map_err(GitSourceError::Store)?;
        let keys = match platform::read_user_docker_file(&directory.join(JOURNAL), MAX_BYTES, true)
            .await
        {
            Ok(bytes) => decode(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(GitSourceError::Store(error)),
        };
        Ok(Self {
            directory: directory.to_owned(),
            keys,
        })
    }

    /// Commit the precise keys potentially affected by the next mutation.
    pub(super) async fn record(
        &mut self,
        keys: impl IntoIterator<Item = String>,
        lock: &Arc<File>,
    ) -> Result<(), GitSourceError> {
        let mut next = self.keys.clone();
        for key in keys {
            if !valid_key(&key) {
                return Err(invalid_journal());
            }
            next.insert(key);
            if next.len() > MAX_KEYS {
                return Err(invalid_journal());
            }
        }
        self.replace(next, lock).await
    }

    /// Remove active keys from the journal and retry a bounded batch of obsolete keys.
    pub(super) async fn reconcile(
        &mut self,
        active: &HashSet<String>,
        lock: &Arc<File>,
    ) -> Result<(), GitSourceError> {
        self.reconcile_with(active, lock, erase_key).await
    }

    async fn reconcile_with(
        &mut self,
        active: &HashSet<String>,
        lock: &Arc<File>,
        mut erase: impl AsyncFnMut(&str) -> Result<(), GitSourceError>,
    ) -> Result<(), GitSourceError> {
        let mut next = self.keys.clone();
        next.retain(|key| !active.contains(key));
        for key in self
            .keys
            .iter()
            .filter(|key| !active.contains(*key))
            .take(MAX_ERASES)
        {
            if erase(key).await.is_err() {
                break;
            }
            next.remove(key);
        }
        let pending = !next.is_empty();
        self.replace(next, lock).await?;
        if pending {
            return Err(GitSourceError::CredentialCleanup);
        }
        Ok(())
    }

    async fn replace(
        &mut self,
        next: BTreeSet<String>,
        lock: &Arc<File>,
    ) -> Result<(), GitSourceError> {
        if self.keys == next {
            return Ok(());
        }
        let data = serde_json::to_vec(&next)
            .map_err(io::Error::other)
            .map_err(GitSourceError::Store)?;
        if data.len() > MAX_BYTES {
            return Err(invalid_journal());
        }

        let directory = self.directory.clone();
        let lock = Arc::clone(lock);
        tokio::task::spawn_blocking(move || {
            // Keep the mutation locked until the blocking write finishes, even
            // when the caller is cancelled while waiting for the write.
            let _lock = lock;
            let parent = File::open(&directory)?;
            platform::replace_docker_state(&directory.join(JOURNAL), &data)?;
            parent.sync_all()
        })
        .await
        .map_err(io::Error::other)
        .map_err(GitSourceError::Store)?
        .map_err(GitSourceError::Store)?;

        self.keys = next;
        Ok(())
    }
}

fn decode(bytes: &[u8]) -> Result<BTreeSet<String>, GitSourceError> {
    let entries: Vec<String> = serde_json::from_slice(bytes)
        .map_err(io::Error::other)
        .map_err(GitSourceError::Store)?;
    if entries.len() > MAX_KEYS || entries.iter().any(|key| !valid_key(key)) {
        return Err(invalid_journal());
    }
    let count = entries.len();
    let keys: BTreeSet<_> = entries.into_iter().collect();
    if keys.len() != count {
        return Err(invalid_journal());
    }
    Ok(keys)
}

fn invalid_journal() -> GitSourceError {
    GitSourceError::Store(io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or full Git credential cleanup journal",
    ))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    use super::*;

    fn key(sequence: usize) -> String {
        let mut digest = [0_u8; 32];
        digest[..std::mem::size_of::<usize>()].copy_from_slice(&sequence.to_be_bytes());
        format!(
            "https://git-sources.marsa.invalid/v1/{}",
            URL_SAFE_NO_PAD.encode(digest)
        )
    }

    #[test]
    fn rejects_foreign_duplicate_and_excess_journal_keys() {
        for invalid in [
            vec!["https://registries.marsa.invalid/v1/not-a-git-key".to_owned()],
            vec![key(1), key(1)],
            (0..=MAX_KEYS).map(key).collect(),
        ] {
            assert!(matches!(
                decode(&serde_json::to_vec(&invalid).unwrap()),
                Err(GitSourceError::Store(_))
            ));
        }
    }

    #[tokio::test]
    async fn rejects_a_public_or_symlinked_journal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(JOURNAL);
        let contents = serde_json::to_vec(&vec![key(1)]).unwrap();
        std::fs::write(&path, &contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Journal::load(directory.path()).await.is_err());

        let private_file = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        std::fs::write(private_file.path(), &contents).unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(private_file.path(), &path).unwrap();
        assert!(Journal::load(directory.path()).await.is_err());
    }

    #[tokio::test]
    async fn reconciliation_preserves_failed_and_unprocessed_keys_after_reload() {
        let directory = tempfile::tempdir().unwrap();
        let lock = Arc::new(platform::lock_docker_state(directory.path(), "fixture.lock").unwrap());
        let mut journal = Journal::load(directory.path()).await.unwrap();
        journal.record((0..8).map(key), &lock).await.unwrap();

        let ordered: Vec<_> = journal.keys.iter().cloned().collect();
        let active = HashSet::from([ordered[0].clone()]);
        let mut attempted = Vec::new();
        let result = journal
            .reconcile_with(&active, &lock, async |target| {
                attempted.push(target.to_owned());
                if target == ordered[2] {
                    return Err(GitSourceError::CredentialUnavailable);
                }
                Ok(())
            })
            .await;
        assert!(matches!(result, Err(GitSourceError::CredentialCleanup)));
        assert_eq!(attempted, ordered[1..3]);
        let mut journal = Journal::load(directory.path()).await.unwrap();
        assert_eq!(journal.keys, ordered[2..].iter().cloned().collect());

        attempted.clear();
        assert!(matches!(
            journal
                .reconcile_with(&active, &lock, async |target| {
                    attempted.push(target.to_owned());
                    Ok(())
                })
                .await,
            Err(GitSourceError::CredentialCleanup)
        ));
        assert_eq!(attempted, ordered[2..2 + MAX_ERASES]);
        assert_eq!(
            Journal::load(directory.path()).await.unwrap().keys,
            ordered[2 + MAX_ERASES..].iter().cloned().collect()
        );
        journal
            .reconcile_with(&active, &lock, async |_| Ok(()))
            .await
            .unwrap();
        assert!(
            Journal::load(directory.path())
                .await
                .unwrap()
                .keys
                .is_empty()
        );
    }
}
