//! Offer one Keychain credential to trusted Git over an owner-private socket.

use std::{fs::Permissions, io, os::unix::fs::PermissionsExt, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hyper::http::uri::Authority;
use nix::unistd::geteuid;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    net::{UnixListener, UnixStream},
    process::Command,
    task::JoinHandle,
    time::timeout,
};

use super::{GitRepository, GitSourceError};
use crate::{
    docker::keychain::{self, Credential},
    platform,
};

const PREFIX: &str = "https://git-sources.marsa.invalid/v1/";
const REQUEST_LIMIT: usize = 8 * 1024;
const CREDENTIAL_HELPER: &str = r#"credential.helper=!f() { if test "$1" = get; then exec /usr/bin/nc -U "$MARSA_GIT_CREDENTIAL_SOCKET"; fi; }; f"#;

pub(super) fn key(repository: &GitRepository, revision: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(repository.id.as_bytes());
    digest.update(b"\0");
    digest.update(repository.config.url.as_bytes());
    digest.update(b"\0");
    digest.update(revision.as_bytes());
    format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(digest.finalize()))
}

pub(super) fn valid_values(username: &str, secret: &str) -> bool {
    valid_value(username, 256) && !username.contains(':') && valid_value(secret, 4096)
}

fn valid_value(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.contains(['\0', '\r', '\n'])
}

pub(super) async fn save(
    repository: &GitRepository,
    revision: &str,
    username: String,
    secret: String,
) -> Result<(), GitSourceError> {
    if !valid_values(&username, &secret) || Context::for_url(&repository.config.url).is_none() {
        return Err(GitSourceError::CredentialInvalid);
    }
    let helper = provider().await?;
    let credential = Credential {
        server_url: key(repository, revision),
        username,
        secret,
    };
    keychain::store(&helper, &credential)
        .await
        .map_err(|_| GitSourceError::CredentialUnavailable)
}

pub(super) async fn erase(
    repository: &GitRepository,
    revision: &str,
) -> Result<(), GitSourceError> {
    let helper = provider().await?;
    keychain::erase(&helper, &key(repository, revision))
        .await
        .map_err(|_| GitSourceError::CredentialUnavailable)
}

async fn provider() -> Result<std::path::PathBuf, GitSourceError> {
    if geteuid().is_root() {
        return Err(GitSourceError::CredentialUnavailable);
    }
    platform::bundled_registry_helper()
        .await
        .map_err(|_| GitSourceError::CredentialUnavailable)
}

pub(super) async fn attach(
    command: &mut Command,
    repository: Option<&GitRepository>,
) -> Result<Option<Broker>, GitSourceError> {
    let Some((repository, revision)) = repository
        .and_then(|repository| Some((repository, repository.credential_revision.as_deref()?)))
    else {
        return Ok(None);
    };
    let helper = provider().await?;
    let credential = keychain::get(&helper, &key(repository, revision))
        .await
        .map_err(|_| GitSourceError::CredentialUnavailable)?
        .filter(|credential| valid_values(&credential.username, &credential.secret))
        .ok_or(GitSourceError::CredentialUnavailable)?;
    let expected = Context::for_url(&repository.config.url).ok_or(GitSourceError::Invalid)?;
    let broker = Broker::new(expected, credential).await?;
    command
        .env("MARSA_GIT_CREDENTIAL_SOCKET", broker.socket())
        .args(["-c", "credential.useHttpPath=true", "-c", CREDENTIAL_HELPER]);
    Ok(Some(broker))
}

pub(super) struct Broker {
    directory: tempfile::TempDir,
    task: JoinHandle<()>,
}

impl Broker {
    async fn new(expected: Context, credential: Credential) -> Result<Self, GitSourceError> {
        let directory = tokio::task::spawn_blocking(|| {
            tempfile::Builder::new()
                .prefix("mgit.")
                .permissions(Permissions::from_mode(0o700))
                .tempdir_in("/tmp")
        })
        .await
        .map_err(|_| GitSourceError::CredentialUnavailable)?
        .map_err(|_| GitSourceError::CredentialUnavailable)?;
        let listener = UnixListener::bind(directory.path().join("ask"))
            .map_err(|_| GitSourceError::CredentialUnavailable)?;
        let task = tokio::spawn(async move {
            for _ in 0..8 {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let _ = timeout(
                    Duration::from_secs(2),
                    answer(stream, &expected, &credential),
                )
                .await;
            }
        });
        Ok(Self { directory, task })
    }

    fn socket(&self) -> std::path::PathBuf {
        self.directory.path().join("ask")
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(stream: UnixStream, expected: &Context, credential: &Credential) -> io::Result<()> {
    if stream.peer_cred()?.uid() != geteuid().as_raw() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut socket = BufReader::new(stream.take((REQUEST_LIMIT + 1) as u64));
    let mut record = Vec::new();
    loop {
        let start = record.len();
        let count = socket.read_until(b'\n', &mut record).await?;
        if record.len() > REQUEST_LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if count == 0 || &record[start..] == b"\n" {
            break;
        }
    }

    let mut stream = socket.into_inner().into_inner();
    if Context::from_request(&record).as_ref() == Some(expected) {
        stream.write_all(b"username=").await?;
        stream.write_all(credential.username.as_bytes()).await?;
        stream.write_all(b"\npassword=").await?;
        stream.write_all(credential.secret.as_bytes()).await?;
        stream.write_all(b"\n\n").await?;
    }
    stream.shutdown().await
}

#[derive(Debug, PartialEq, Eq)]
struct Context {
    host: String,
    port: u16,
    path: String,
}

impl Context {
    fn for_url(url: &str) -> Option<Self> {
        let uri: hyper::Uri = url.parse().ok()?;
        if uri.scheme_str() != Some("https") {
            return None;
        }
        Self::from_parts(uri.authority()?, uri.path().strip_prefix('/')?)
    }

    fn from_parts(authority: &Authority, path: &str) -> Option<Self> {
        if path.is_empty()
            || path.starts_with('/')
            || path.ends_with('/')
            || authority.as_str().contains('@')
        {
            return None;
        }
        let host = authority.host();
        let port = match authority.as_str().strip_prefix(host)? {
            "" => 443,
            port => port
                .strip_prefix(':')?
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)?,
        };
        Some(Self {
            host: host.to_ascii_lowercase(),
            port,
            path: path.to_owned(),
        })
    }

    fn from_request(bytes: &[u8]) -> Option<Self> {
        // Git's credential helper subprocess terminates the request with EOF.
        // The credential CLI also allows clients to send a blank terminator.
        let data = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
        let data = data.strip_suffix('\n').unwrap_or(data);
        let mut protocol = None;
        let mut authority = None;
        let mut path = None;
        for line in data.split('\n') {
            let (name, value) = line.split_once('=')?;
            let target = match name {
                "protocol" => &mut protocol,
                "host" => &mut authority,
                "path" => &mut path,
                "url" => return None,
                _ => continue,
            };
            if target.replace(value).is_some() {
                return None;
            }
        }
        if protocol != Some("https") {
            return None;
        }
        let host = authority?.parse().ok()?;
        Self::from_parts(&host, path?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_requests_must_match_the_exact_saved_https_repository() {
        let expected = Context::for_url("https://example.invalid/team/source.git").unwrap();
        let request = b"protocol=https\nhost=EXAMPLE.invalid:443\npath=team/source.git\n\n";
        assert_eq!(Context::from_request(request).as_ref(), Some(&expected));
        for wrong in [
            "protocol=http\nhost=example.invalid\npath=team/source.git\n\n",
            "protocol=https\nhost=another.invalid\npath=team/source.git\n\n",
            "protocol=https\nhost=example.invalid:444\npath=team/source.git\n\n",
            "protocol=https\nhost=example.invalid:bad\npath=team/source.git\n\n",
            "protocol=https\nhost=example.invalid\npath=team/other.git\n\n",
            "protocol=https\nhost=example.invalid\npath=/team/source.git\n\n",
            "protocol=https\nhost=example.invalid\npath=team/source.git/\n\n",
            "protocol=https\nhost=example.invalid\npath=team/source.git/info/lfs\n\n",
            "protocol=https\nhost=example.invalid\nhost=another.invalid\npath=team/source.git\n\n",
            "protocol=https\nhost=example.invalid\npath=team/source.git\nurl=https://another.invalid/team/source.git\n\n",
            "protocol=https\nhost=example.invalid\n\n",
        ] {
            assert_ne!(
                Context::from_request(wrong.as_bytes()).as_ref(),
                Some(&expected)
            );
        }
        assert!(!valid_values("fixture", "test\nurl=invalid"));
        assert!(!valid_values("fixture:other", "token"));
        assert!(Context::for_url("https://example.invalid//team/source.git").is_none());
        assert!(Context::for_url("https://example.invalid/team/source.git/").is_none());
    }

    #[tokio::test]
    async fn private_broker_withholds_credentials_from_other_repository_requests() {
        let expected = Context::for_url("https://example.invalid/team/source.git").unwrap();
        let dummy = Credential {
            server_url: String::new(),
            username: "fixture".to_owned(),
            secret: "mock-for-protocol-test".to_owned(),
        };
        let broker = Broker::new(expected, dummy).await.unwrap();
        let query = |name: &'static str| {
            let socket = broker.socket();
            async move {
                let mut stream = UnixStream::connect(socket).await.unwrap();
                stream
                    .write_all(
                        format!("protocol=https\nhost=example.invalid\npath=team/{name}.git\n\n")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut received = String::new();
                stream.read_to_string(&mut received).await.unwrap();
                received
            }
        };
        assert!(query("other").await.is_empty());
        assert_eq!(
            query("source").await,
            "username=fixture\npassword=mock-for-protocol-test\n\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn apple_git_uses_the_private_broker_for_only_the_matching_path() {
        use std::process::Stdio;

        let expected = Context::for_url("https://example.invalid/team/source.git").unwrap();
        let dummy = Credential {
            server_url: String::new(),
            username: "fixture".to_owned(),
            secret: "mock-for-apple-git".to_owned(),
        };
        let broker = Broker::new(expected, dummy).await.unwrap();
        for (path, permitted) in [("team/source.git", true), ("team/other.git", false)] {
            let mut git = Command::new("/usr/bin/git")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("MARSA_GIT_CREDENTIAL_SOCKET", broker.socket())
                .args(["-c", "credential.useHttpPath=true", "-c", CREDENTIAL_HELPER])
                .args(["credential", "fill"])
                .kill_on_drop(true)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            git.stdin
                .take()
                .unwrap()
                .write_all(
                    format!("protocol=https\nhost=example.invalid\npath={path}\n\n").as_bytes(),
                )
                .await
                .unwrap();
            let output = timeout(Duration::from_secs(10), git.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(output.status.success(), permitted);
            let output = String::from_utf8(output.stdout).unwrap();
            assert_eq!(output.contains("username=fixture\n"), permitted);
            assert_eq!(output.contains("password=mock-for-apple-git\n"), permitted);
        }
    }
}
