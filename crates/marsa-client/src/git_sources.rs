//! Saved Git references for owner-selected project sources.

use std::{
    collections::HashSet,
    fs::{File, Permissions},
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{fs, process::Command, time::timeout};

use crate::{
    docker_socket_path,
    platform::{self, git_checkout::GitCheckoutDestination},
    request_id,
};

mod credential_cleanup;
mod https_credentials;
mod refs;
mod ssh;

pub use ssh::{GitSshAuthentication, git_ssh_host_key_fingerprint, review_git_ssh_identity};

use credential_cleanup::Journal;

const STORE: &str = "git-repositories.json";
const STORE_LIMIT: usize = 256 * 1024;
const MAX_REPOSITORIES: usize = 128;

/// The editable name, transport and branch of one saved Git source.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitRepositoryConfig {
    /// The user's name for this repository.
    pub name: String,
    /// The URL without embedded credentials, query, or fragment.
    pub url: String,
    /// The branch name, without the refs/heads prefix.
    pub branch: String,
    /// Explicit SSH host key, file reference, and reviewed identity fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<GitSshAuthentication>,
}

/// One saved Git reference with a stable identifier.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitRepository {
    /// The identifier used to edit and remove this saved repository.
    pub id: String,
    /// Transport and display settings.
    pub config: GitRepositoryConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_revision: Option<String>,
}

impl GitRepository {
    /// Whether this exact saved URL has a Keychain association.
    pub fn has_https_credential(&self) -> bool {
        self.credential_revision.is_some()
    }

    /// Test the saved branch using only the credential associated with this URL.
    pub async fn test(&self) -> Result<(), GitSourceError> {
        let active = GitRepositoryStore::discover()?.active(self).await?;
        active.config.test_with(Some(&active)).await
    }

    /// Check out the saved branch using only the credential associated with this URL.
    pub async fn checkout(&self, parent: &Path) -> Result<PathBuf, GitSourceError> {
        let active = GitRepositoryStore::discover()?.active(self).await?;
        active.config.checkout_with(parent, Some(&active)).await
    }
}

/// A local Git reference or operation could not be completed.
#[derive(Debug, thiserror::Error)]
pub enum GitSourceError {
    /// The source name, URL, or branch did not pass validation.
    #[error("expected a name, supported repository URL, and Git branch")]
    Invalid,
    /// The selected repository is no longer saved.
    #[error("the saved Git repository could not be found")]
    NotFound,
    /// The per-user settings could not be read or written.
    #[error("could not access Git repository settings")]
    Store(#[source] io::Error),
    /// The explicitly selected checkout parent was unsuitable.
    #[error("could not create a checkout in the selected folder")]
    Destination,
    /// The installed Git client is inaccessible or failed the run.
    #[error("Git could not verify the branch or complete the checkout")]
    Git,
    /// The process exceeded its deadline.
    #[error("the Git operation timed out")]
    Timeout,
    /// The selected branch did not appear in the remote references.
    #[error("the repository did not advertise the selected branch")]
    BranchMissing,
    /// The username or secret cannot be passed through Git's credential protocol.
    #[error("expected a non-empty HTTPS username and password or token")]
    CredentialInvalid,
    /// The signed helper or matching Keychain item was unavailable.
    #[error("the saved Git credential is unavailable")]
    CredentialUnavailable,
    /// The saved HTTPS credential changed after the row was reviewed.
    #[error("the saved Git credential changed; review the current entry")]
    CredentialChanged,
    /// The association changed and an obsolete Keychain value still needs cleanup.
    #[error("the Git credential changed but the previous Keychain entry remains")]
    CredentialCleanup,
    /// The provided host key cannot establish the required pinned SSH trust.
    #[error("expected a supported, independently verified SSH public host key")]
    SshHostKey,
    /// The selected private file could not be read securely or used noninteractively.
    #[error("choose an owner-only private SSH key that does not require a passphrase prompt")]
    SshIdentity,
    /// The selected private file no longer holds the reviewed key.
    #[error("the private SSH key changed; choose it again to review the new identity")]
    SshIdentityChanged,
    /// SSH could not verify the pinned host or authenticate with the selected identity.
    #[error("SSH could not verify the saved host key or authenticate with the selected key")]
    SshConnection,
    /// SSH checkout cannot be published atomically across filesystems.
    #[error("choose a checkout folder on the same filesystem as Marsa's state")]
    SshDestinationFilesystem,
}

enum Remote {
    Https {
        _broker: Option<https_credentials::Broker>,
    },
    Ssh {
        _session: ssh::Session,
    },
}

impl GitRepositoryConfig {
    /// Validate the saved display name and restricted remote reference.
    pub fn validate(&self) -> Result<(), GitSourceError> {
        let safe_name = !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.chars().any(|ch| {
                ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            });
        if !safe_name || !valid_branch(&self.branch) {
            return Err(GitSourceError::Invalid);
        }
        match &self.ssh {
            None if valid_url(&self.url) => Ok(()),
            None if ssh::Address::parse(&self.url).is_some() => Err(GitSourceError::SshIdentity),
            Some(authentication) if ssh::Address::parse(&self.url).is_some() => {
                authentication.validate()
            }
            _ => Err(GitSourceError::Invalid),
        }
    }

    async fn remote(
        &self,
        command: &mut Command,
        repository: Option<&GitRepository>,
    ) -> Result<Remote, GitSourceError> {
        if let Some(authentication) = &self.ssh {
            let session = ssh::attach(command, &self.url, authentication).await?;
            Ok(Remote::Ssh { _session: session })
        } else {
            let credential = https_credentials::attach(command, repository).await?;
            Ok(Remote::Https {
                _broker: credential,
            })
        }
    }

    fn connection_failure(&self) -> GitSourceError {
        if self.ssh.is_some() {
            GitSourceError::SshConnection
        } else {
            GitSourceError::Git
        }
    }

    /// Ask the validated remote whether it advertises the exact selected branch.
    pub async fn test(&self) -> Result<(), GitSourceError> {
        self.test_with(None).await
    }

    async fn test_with(&self, repository: Option<&GitRepository>) -> Result<(), GitSourceError> {
        self.validate()?;
        let workspace = private_git_directory().await?;
        let home = private_git_directory().await?;
        let mut command = git(workspace.path(), home.path()).await?;
        let _remote = self.remote(&mut command, repository).await?;
        command
            .args(["ls-remote", "--exit-code", "--refs", "--", &self.url])
            .arg(format!("refs/heads/{}", self.branch));
        refs::advertised(&mut command, &self.branch)
            .await
            .map_err(|error| match error {
                GitSourceError::Git => self.connection_failure(),
                other => other,
            })
    }

    /// Clone the selected branch to a fresh private child of the chosen folder.
    ///
    /// The caller can reveal the completed checkout or use a separate,
    /// owner-approved workflow to select a file inside it.
    pub async fn checkout(&self, parent: &Path) -> Result<PathBuf, GitSourceError> {
        self.checkout_with(parent, None).await
    }

    async fn checkout_with(
        &self,
        parent: &Path,
        repository: Option<&GitRepository>,
    ) -> Result<PathBuf, GitSourceError> {
        self.validate()?;
        if !parent.is_absolute() {
            return Err(GitSourceError::Destination);
        }
        let (workspace, publish) = if self.ssh.is_some() {
            let workspace = private_git_directory().await?;
            let destination = GitCheckoutDestination::prepare(parent, workspace.path())
                .await
                .map_err(|error| match error.kind() {
                    io::ErrorKind::CrossesDevices => GitSourceError::SshDestinationFilesystem,
                    _ => GitSourceError::Destination,
                })?;
            (workspace, Some(destination))
        } else {
            let directory = fs::canonicalize(parent)
                .await
                .map_err(|_| GitSourceError::Destination)?;
            platform::check_user_docker_directory(&directory)
                .await
                .map_err(|_| GitSourceError::Destination)?;
            let workspace = tokio::task::spawn_blocking(move || {
                tempfile::Builder::new()
                    .prefix("Marsa checkout ")
                    .permissions(Permissions::from_mode(0o700))
                    .tempdir_in(directory)
            })
            .await
            .map_err(|_| GitSourceError::Destination)?
            .map_err(|_| GitSourceError::Destination)?;
            (workspace, None)
        };
        let home = private_git_directory().await?;
        let mut command = git(workspace.path(), home.path()).await?;
        let remote = self.remote(&mut command, repository).await?;
        command.args([
            "clone",
            "--template=",
            "--no-recurse-submodules",
            "--single-branch",
            "--depth=1",
            "--no-tags",
            "--branch",
            &self.branch,
            "--",
            &self.url,
            ".",
        ]);
        if !run_git(&mut command, Duration::from_secs(120))
            .await?
            .success()
        {
            return Err(self.connection_failure());
        }
        drop(remote);
        refs::checked_out(workspace.path(), &self.branch).await?;
        if let Some(destination) = publish {
            destination
                .publish(workspace)
                .await
                .map_err(|_| GitSourceError::Destination)
        } else {
            Ok(workspace.keep())
        }
    }
}

fn valid_url(value: &str) -> bool {
    if value.len() > 1024 {
        return false;
    }
    let Ok(uri) = value.parse::<hyper::Uri>() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    value.starts_with("https://")
        && uri.scheme_str() == Some("https")
        && uri.query().is_none()
        && !value.contains(['@', '#'])
        && host.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
        && value
            .strip_prefix("https://")
            .and_then(|url| url.split_once('/'))
            .is_some_and(|(authority, _)| {
                authority
                    .split_once(':')
                    .is_none_or(|(_, port)| port.parse::<u16>().is_ok_and(|port| port > 0))
            })
        && uri.path().trim_matches('/').split('/').all(|segment| {
            !segment.is_empty()
                && !matches!(segment, "." | "..")
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-~".contains(&byte))
        })
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && !value.ends_with('.')
        && !value.contains("..")
        && value.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(".lock")
                && part
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
        })
}

async fn private_git_directory() -> Result<tempfile::TempDir, GitSourceError> {
    private_git_directory_in(&GitRepositoryStore::discover()?.0).await
}

async fn private_git_directory_in(directory: &Path) -> Result<tempfile::TempDir, GitSourceError> {
    // The state root is excluded from guest shares. It may not exist yet for
    // a first-use profile, even though the owner has already chosen a source.
    match fs::DirBuilder::new().mode(0o700).create(directory).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(GitSourceError::Store(error)),
    }
    platform::check_user_docker_directory(directory)
        .await
        .map_err(GitSourceError::Store)?;
    let directory = directory.to_owned();
    tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix("git-operation-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(directory)
    })
    .await
    .map_err(|_| GitSourceError::Git)?
    .map_err(|_| GitSourceError::Git)
}

async fn git(workspace: &Path, home: &Path) -> Result<Command, GitSourceError> {
    let executable = platform::system_git()
        .await
        .map_err(|_| GitSourceError::Git)?;
    let mut command = Command::new(executable);
    command
        .current_dir(workspace)
        .env_clear()
        // libcurl otherwise falls back to the login user's home for .netrc.
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_CEILING_DIRECTORIES",
            workspace.parent().ok_or(GitSourceError::Git)?,
        )
        .args([
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.sslVerify=true",
            "-c",
            "http.emptyAuth=false",
            "-c",
            "credential.helper=",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "fetch.fsckObjects=true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

async fn run_git(
    command: &mut Command,
    deadline: Duration,
) -> Result<std::process::ExitStatus, GitSourceError> {
    timeout(deadline, command.status())
        .await
        .map_err(|_| GitSourceError::Timeout)?
        .map_err(|_| GitSourceError::Git)
}

/// The owner-private settings file for the desktop's saved Git repositories.
pub struct GitRepositoryStore(PathBuf);

impl GitRepositoryStore {
    /// Find the existing Marsa state directory without connecting to a daemon.
    pub fn discover() -> Result<Self, GitSourceError> {
        let socket =
            docker_socket_path().map_err(|error| GitSourceError::Store(io::Error::other(error)))?;
        let path = socket
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| GitSourceError::Store(io::ErrorKind::InvalidInput.into()))?;
        Ok(Self::at(path.to_owned()))
    }

    pub(crate) fn at(path: PathBuf) -> Self {
        Self(path)
    }

    /// Read the saved repository references in their creation order.
    pub async fn list(&self) -> Result<Vec<GitRepository>, GitSourceError> {
        let result = async {
            platform::check_user_docker_directory(&self.0).await?;
            platform::read_user_docker_file(&self.0.join(STORE), STORE_LIMIT, true).await
        }
        .await;
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(GitSourceError::Store(error)),
        };
        let entries: Vec<GitRepository> = serde_json::from_slice(&bytes)
            .map_err(io::Error::other)
            .map_err(GitSourceError::Store)?;
        let mut ids = HashSet::new();
        if entries.len() > MAX_REPOSITORIES
            || entries.iter().any(|item| {
                !valid_identifier(&item.id)
                    || item.credential_revision.as_deref().is_some_and(|revision| {
                        !valid_identifier(revision) || item.config.ssh.is_some()
                    })
                    || !ids.insert(&item.id)
                    || item.config.validate().is_err()
            })
        {
            return Err(GitSourceError::Store(io::ErrorKind::InvalidData.into()));
        }
        Ok(entries)
    }

    async fn active(&self, selected: &GitRepository) -> Result<GitRepository, GitSourceError> {
        self.list()
            .await?
            .into_iter()
            .find(|entry| {
                entry.id == selected.id
                    && entry.config.url == selected.config.url
                    && entry.config.branch == selected.config.branch
                    && entry.config.ssh == selected.config.ssh
            })
            .ok_or(GitSourceError::NotFound)
    }

    /// Add a reference or edit the matching saved reference while preserving its ID.
    pub async fn save(
        &self,
        id: Option<&str>,
        config: GitRepositoryConfig,
    ) -> Result<String, GitSourceError> {
        config.validate()?;
        self.change(|entries| {
            if let Some(id) = id {
                let item = entries
                    .iter_mut()
                    .find(|item| item.id == id)
                    .ok_or(GitSourceError::NotFound)?;
                let revoke = if item.config.url != config.url {
                    let key = credential_key(item);
                    item.credential_revision = None;
                    key
                } else {
                    None
                };
                item.config = config;
                return Ok((item.id.clone(), revoke));
            }
            let id = request_id().map_err(GitSourceError::Store)?;
            if entries.len() >= MAX_REPOSITORIES || entries.iter().any(|item| item.id == id) {
                return Err(GitSourceError::Store(io::Error::other(
                    "Git repository store is full or ID was reused",
                )));
            }
            entries.push(GitRepository {
                id: id.clone(),
                config,
                credential_revision: None,
            });
            Ok((id, None))
        })
        .await
    }

    /// Remove a saved reference and revoke its Keychain association.
    pub async fn remove(&self, id: &str) -> Result<(), GitSourceError> {
        self.change(|entries| {
            let index = entries
                .iter()
                .position(|item| item.id == id)
                .ok_or(GitSourceError::NotFound)?;
            Ok(((), credential_key(&entries.remove(index))))
        })
        .await
    }

    /// Store a credential for the currently saved repository and exact approved URL.
    pub async fn save_https_credential(
        &self,
        reviewed: &GitRepository,
        username: String,
        secret: String,
    ) -> Result<(), GitSourceError> {
        if reviewed.config.ssh.is_some() || !https_credentials::valid_values(&username, &secret) {
            return Err(GitSourceError::CredentialInvalid);
        }
        let lock = self.lock().await?;
        let mut entries = self.list().await?;
        let active = credential_keys(&entries);
        let item = reviewed_credential(&mut entries, reviewed)?;
        let revision = request_id().map_err(GitSourceError::Store)?;
        let mut journal = Journal::load(&self.0).await?;
        if journal.needs_capacity() {
            let _ = journal.reconcile(&active, &lock).await;
        }
        // Track both outcomes before writing Keychain. Recovery preserves
        // whichever revision the repository file ultimately commits.
        let key = https_credentials::key(item, &revision);
        journal
            .record(std::iter::once(key).chain(credential_key(item)), &lock)
            .await?;
        https_credentials::save(item, &revision, username, secret).await?;
        item.credential_revision = Some(revision);
        self.finish_change(&entries, &mut journal, &lock).await
    }

    /// Revoke the Keychain association for the exact saved source.
    pub async fn remove_https_credential(
        &self,
        reviewed: &GitRepository,
    ) -> Result<(), GitSourceError> {
        self.change(|entries| {
            let item = reviewed_credential(entries, reviewed)?;
            let old = credential_key(item);
            item.credential_revision = None;
            Ok(((), old))
        })
        .await
    }

    async fn change<T>(
        &self,
        action: impl FnOnce(&mut Vec<GitRepository>) -> Result<(T, Option<String>), GitSourceError>,
    ) -> Result<T, GitSourceError> {
        let lock = self.lock().await?;
        let mut entries = self.list().await?;
        let active = credential_keys(&entries);
        let (result, revoke) = action(&mut entries)?;
        let mut journal = Journal::load(&self.0).await?;
        if journal.needs_capacity() {
            let _ = journal.reconcile(&active, &lock).await;
        }
        journal.record(revoke, &lock).await?;
        self.finish_change(&entries, &mut journal, &lock).await?;
        Ok(result)
    }

    async fn finish_change(
        &self,
        entries: &[GitRepository],
        journal: &mut Journal,
        lock: &Arc<File>,
    ) -> Result<(), GitSourceError> {
        self.persist(entries, lock).await?;
        journal
            .reconcile(&credential_keys(entries), lock)
            .await
            .map_err(|_| GitSourceError::CredentialCleanup)
    }

    async fn lock(&self) -> Result<Arc<File>, GitSourceError> {
        let directory = self.0.clone();
        tokio::task::spawn_blocking(move || {
            platform::lock_docker_state(&directory, "desktop-git-sources.lock")
        })
        .await
        .map_err(io::Error::other)
        .map_err(GitSourceError::Store)?
        .map_err(GitSourceError::Store)
        .map(Arc::new)
    }

    async fn persist(
        &self,
        entries: &[GitRepository],
        lock: &Arc<File>,
    ) -> Result<(), GitSourceError> {
        let data = serde_json::to_vec(entries)
            .map_err(io::Error::other)
            .map_err(GitSourceError::Store)?;
        if data.len() > STORE_LIMIT {
            return Err(GitSourceError::Store(io::ErrorKind::InvalidData.into()));
        }
        let directory = self.0.clone();
        let path = directory.join(STORE);
        let lock = lock.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            platform::replace_docker_state(&path, &data)?;
            // Erasing the former Keychain item must follow a durable commit.
            File::open(directory)?.sync_all()
        })
        .await
        .map_err(io::Error::other)
        .map_err(GitSourceError::Store)?
        .map_err(GitSourceError::Store)
    }
}

fn reviewed_credential<'a>(
    entries: &'a mut [GitRepository],
    reviewed: &GitRepository,
) -> Result<&'a mut GitRepository, GitSourceError> {
    let item = entries
        .iter_mut()
        .find(|item| item.id == reviewed.id)
        .ok_or(GitSourceError::NotFound)?;
    if item.config.url != reviewed.config.url
        || item.credential_revision != reviewed.credential_revision
    {
        return Err(GitSourceError::CredentialChanged);
    }
    Ok(item)
}

fn valid_identifier(value: &str) -> bool {
    value.len() == 36
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn credential_key(repository: &GitRepository) -> Option<String> {
    repository
        .credential_revision
        .as_deref()
        .map(|revision| https_credentials::key(repository, revision))
}

fn credential_keys(entries: &[GitRepository]) -> HashSet<String> {
    entries.iter().filter_map(credential_key).collect()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn protected_git_state_rejects_symlinked_and_writable_roots() {
        let owner = tempfile::tempdir().unwrap();
        let outside = owner.path().join("other");
        let state = owner.path().join(".marsa");
        fs::create_dir(&outside).await.unwrap();
        std::os::unix::fs::symlink(&outside, &state).unwrap();
        assert!(private_git_directory_in(&state).await.is_err());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_file(&state).await.unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&state)
            .await
            .unwrap();
        fs::set_permissions(&state, Permissions::from_mode(0o777))
            .await
            .unwrap();
        assert!(private_git_directory_in(&state).await.is_err());
        assert!(std::fs::read_dir(&state).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn git_uses_an_empty_home_outside_the_checkout() {
        let owner = tempfile::tempdir().unwrap();
        let state = owner.path().join(".marsa");
        let checkout = private_git_directory_in(&state).await.unwrap();
        let home = private_git_directory_in(&state).await.unwrap();
        assert_eq!(home.path().parent(), Some(state.as_path()));
        fs::write(checkout.path().join(".netrc"), "macros in the checkout")
            .await
            .unwrap();
        let git = git(checkout.path(), home.path()).await.unwrap();
        let actual = git
            .as_std()
            .get_envs()
            .find_map(|(key, value)| (key == "HOME").then_some(value).flatten());
        assert_eq!(actual, Some(home.path().as_os_str()));
        assert_ne!(git.as_std().get_current_dir(), Some(home.path()));
        assert!(!home.path().join(".netrc").exists());
    }
}
