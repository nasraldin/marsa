//! Saved HTTPS Git references for owner-selected project sources.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{fs, process::Command, time::timeout};

use crate::{docker_socket_path, platform, request_id};

const STORE: &str = "git-repositories.json";
const STORE_LIMIT: usize = 256 * 1024;
const MAX_REPOSITORIES: usize = 128;

/// The editable name, HTTPS URL and branch of one saved Git source.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitRepositoryConfig {
    /// The user's name for this repository.
    pub name: String,
    /// The URL without embedded credentials, query, or fragment.
    pub url: String,
    /// The branch name, without the refs/heads prefix.
    pub branch: String,
}

/// One saved Git reference with a stable identifier.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitRepository {
    /// The identifier used to edit and remove this saved repository.
    pub id: String,
    /// Transport and display settings.
    pub config: GitRepositoryConfig,
}

/// A local Git reference or operation could not be completed.
#[derive(Debug, thiserror::Error)]
pub enum GitSourceError {
    /// The source name, URL, or branch did not pass validation.
    #[error("expected a name, HTTPS repository URL, and Git branch")]
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
}

impl GitRepositoryConfig {
    /// Validate the saved display name and restricted remote reference.
    pub fn validate(&self) -> Result<(), GitSourceError> {
        let safe_name = !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.chars().any(|ch| {
                ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            });
        if safe_name && valid_url(&self.url) && valid_branch(&self.branch) {
            Ok(())
        } else {
            Err(GitSourceError::Invalid)
        }
    }

    /// Ask the validated remote whether it advertises the exact selected branch.
    pub async fn test(&self) -> Result<(), GitSourceError> {
        self.validate()?;
        let workspace = tokio::task::spawn_blocking(tempfile::tempdir)
            .await
            .map_err(|_| GitSourceError::Git)?
            .map_err(|_| GitSourceError::Git)?;
        let mut command = git(workspace.path()).await?;
        command
            .args(["ls-remote", "--exit-code", "--refs", "--", &self.url])
            .arg(format!("refs/heads/{}", self.branch));
        match run_git(&mut command, Duration::from_secs(20)).await? {
            status if status.success() => Ok(()),
            status if status.code() == Some(2) => Err(GitSourceError::BranchMissing),
            _ => Err(GitSourceError::Git),
        }
    }

    /// Clone the selected branch to a fresh private child of the chosen folder.
    ///
    /// The caller can reveal the completed checkout or use a separate,
    /// owner-approved workflow to select a file inside it.
    pub async fn checkout(&self, parent: &Path) -> Result<PathBuf, GitSourceError> {
        self.validate()?;
        if !parent.is_absolute() {
            return Err(GitSourceError::Destination);
        }
        let directory = fs::canonicalize(parent)
            .await
            .map_err(|_| GitSourceError::Destination)?;
        platform::check_user_docker_directory(&directory)
            .await
            .map_err(|_| GitSourceError::Destination)?;
        let workspace = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .prefix("Marsa checkout ")
                .tempdir_in(directory)
        })
        .await
        .map_err(|_| GitSourceError::Destination)?
        .map_err(|_| GitSourceError::Destination)?;
        let mut command = git(workspace.path()).await?;
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
        if run_git(&mut command, Duration::from_secs(120))
            .await?
            .success()
        {
            Ok(workspace.keep())
        } else {
            Err(GitSourceError::Git)
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

async fn git(workspace: &Path) -> Result<Command, GitSourceError> {
    let executable = platform::system_git()
        .await
        .map_err(|_| GitSourceError::Git)?;
    let mut command = Command::new(executable);
    command
        .current_dir(workspace)
        .env_clear()
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
                item.id.len() != 36
                    || !item
                        .id
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
                    || !ids.insert(&item.id)
                    || item.config.validate().is_err()
            })
        {
            return Err(GitSourceError::Store(io::ErrorKind::InvalidData.into()));
        }
        Ok(entries)
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
                item.config = config;
                return Ok(item.id.clone());
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
            });
            Ok(id)
        })
        .await
    }

    /// Remove a saved reference. No checkouts are removed.
    pub async fn remove(&self, id: &str) -> Result<(), GitSourceError> {
        self.change(|entries| {
            let index = entries
                .iter()
                .position(|item| item.id == id)
                .ok_or(GitSourceError::NotFound)?;
            entries.remove(index);
            Ok(())
        })
        .await
    }

    async fn change<T>(
        &self,
        action: impl FnOnce(&mut Vec<GitRepository>) -> Result<T, GitSourceError>,
    ) -> Result<T, GitSourceError> {
        let directory = self.0.clone();
        let lock = tokio::task::spawn_blocking(move || {
            platform::lock_docker_state(&directory, "desktop-git-sources.lock")
        })
        .await
        .map_err(io::Error::other)
        .map_err(GitSourceError::Store)?
        .map_err(GitSourceError::Store)?;
        let mut entries = self.list().await?;
        let result = action(&mut entries)?;
        let data = serde_json::to_vec(&entries)
            .map_err(io::Error::other)
            .map_err(GitSourceError::Store)?;
        if data.len() > STORE_LIMIT {
            return Err(GitSourceError::Store(io::ErrorKind::InvalidData.into()));
        }
        let path = self.0.join(STORE);
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            platform::replace_docker_state(&path, &data)
        })
        .await
        .map_err(io::Error::other)
        .map_err(GitSourceError::Store)?
        .map_err(GitSourceError::Store)?;
        Ok(result)
    }
}
