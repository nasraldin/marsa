//! Pin Git's SSH host and client key in a fresh per-operation OpenSSH configuration.

use std::{
    io,
    net::Ipv6Addr,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    fs,
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::{Child, ChildStdin, Command},
    time::{sleep, timeout},
};

use super::GitSourceError;
use crate::platform;

const SSH: &str = "/usr/bin/ssh";
const AGENT: &str = "/usr/bin/ssh-agent";
const ADD: &str = "/usr/bin/ssh-add";
const LIFETIME: &str = "150";
const SOCKET: &str = "./agent";
// EOF arrives on normal drop and on process death. The child SSH agent
// stays in this directory and the supervisor only kills its own child.
const AGENT_SUPERVISOR: &str = r#"
    /usr/bin/ssh-agent -D -a ./agent -t "$1" >/dev/null 2>&1 &
    agent=$!
    trap 'kill "$agent" 2>/dev/null || :; wait "$agent" 2>/dev/null || :' EXIT
    IFS= read -r lease || :
"#;
const HOST_ALIAS: &str = "marsa-pinned-git-source";
const KEY_LIMIT: usize = 32 * 1024;
const PUBLIC_KEY_LIMIT: usize = 8 * 1024;

/// Out-of-band SSH host trust and the selected owner's private-key reference.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitSshAuthentication {
    /// OpenSSH public host key obtained through a trusted channel.
    pub host_key: String,
    /// Owner-private key file chosen in Settings.
    pub identity_file: PathBuf,
    /// SHA-256 fingerprint of the public half of the reviewed identity.
    pub identity_fingerprint: String,
}

/// Format the fingerprint of an OpenSSH public key independently of its comment.
pub fn git_ssh_host_key_fingerprint(value: &str) -> Option<String> {
    Some(PublicKey::parse(value)?.fingerprint())
}

/// Review a selected private-key file and return its public fingerprint without opening SSH.
pub async fn review_git_ssh_identity(path: &Path) -> Result<String, GitSourceError> {
    Ok(Identity::read(path).await?.fingerprint)
}

impl GitSshAuthentication {
    pub(super) fn validate(&self) -> Result<(), GitSourceError> {
        if PublicKey::parse(&self.host_key).is_none() {
            return Err(GitSourceError::SshHostKey);
        }
        if !valid_identity_path(&self.identity_file)
            || !self
                .identity_fingerprint
                .strip_prefix("SHA256:")
                .and_then(|value| STANDARD_NO_PAD.decode(value).ok())
                .is_some_and(|digest| digest.len() == 32)
        {
            return Err(GitSourceError::SshIdentity);
        }
        Ok(())
    }
}

pub(super) struct Address<'a> {
    user: &'a str,
    host: &'a str,
    port: u16,
    path: &'a str,
}

impl<'a> Address<'a> {
    pub(super) fn parse(value: &'a str) -> Option<Self> {
        if value.len() > 1024 {
            return None;
        }
        let (authority, path) = value.strip_prefix("ssh://")?.split_once('/')?;
        let (user, server) = authority.split_once('@')?;
        if user.is_empty()
            || user.len() > 64
            || !user
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            || !user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !path.split('/').all(|part| {
                !part.is_empty()
                    && !matches!(part, "." | "..")
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        {
            return None;
        }
        let (host, suffix) = if let Some(ipv6) = server.strip_prefix('[') {
            let (ip, suffix) = ipv6.split_once(']')?;
            ip.parse::<Ipv6Addr>().ok()?;
            (ip, suffix)
        } else {
            let (host, suffix) = server
                .find(':')
                .map_or((server, ""), |at| server.split_at(at));
            if !host.split('.').all(|label| {
                !label.is_empty()
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.len() <= 63
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }) {
                return None;
            }
            (host, suffix)
        };
        let port = match suffix {
            "" => 22,
            suffix => {
                let port = suffix.strip_prefix(':')?;
                if !port.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                port.parse::<u16>().ok().filter(|port| *port != 0)?
            }
        };
        Some(Self {
            user,
            host,
            port,
            path,
        })
    }
}

pub(super) struct Session {
    _identity: Identity,
}

pub(super) async fn attach(
    command: &mut Command,
    url: &str,
    auth: &GitSshAuthentication,
) -> Result<Session, GitSourceError> {
    let address = Address::parse(url).ok_or(GitSourceError::Invalid)?;
    let host = PublicKey::parse(&auth.host_key).ok_or(GitSourceError::SshHostKey)?;
    trusted_program(SSH).await?;
    let identity = Identity::read(&auth.identity_file).await?;
    if identity.fingerprint != auth.identity_fingerprint {
        return Err(GitSourceError::SshIdentityChanged);
    }
    let base = identity.directory.path();
    let config = base.join("config");
    let known_hosts = base.join("known_hosts");
    write_private(
        &known_hosts,
        format!("{HOST_ALIAS} {} {}\n", host.kind, host.encoded).as_bytes(),
    )
    .await
    .map_err(|_| GitSourceError::SshConnection)?;
    let algorithms = if host.kind == "ssh-rsa" {
        "rsa-sha2-512,rsa-sha2-256"
    } else {
        host.kind
    };
    let configuration = format!(
        concat!(
            "Host {alias}\n",
            "    HostName {hostname}\n",
            "    Port {port}\n",
            "    User {user}\n",
            "    HostKeyAlias {alias}\n",
            "    HostKeyAlgorithms {algorithms}\n",
            "    UserKnownHostsFile ./known_hosts\n",
            "    IdentityFile ./identity.pub\n",
            "    GlobalKnownHostsFile /dev/null\n",
            "    StrictHostKeyChecking yes\n",
            "    VerifyHostKeyDNS no\n",
            "    UpdateHostKeys no\n",
            "    IdentitiesOnly yes\n",
            "    IdentityAgent ./agent\n",
            "    CertificateFile none\n",
            "    AddKeysToAgent no\n",
            "    IgnoreUnknown UseKeychain\n",
            "    UseKeychain no\n",
            "    BatchMode yes\n",
            "    PreferredAuthentications publickey\n",
            "    PasswordAuthentication no\n",
            "    KbdInteractiveAuthentication no\n",
            "    GSSAPIAuthentication no\n",
            "    HostbasedAuthentication no\n",
            "    ProxyCommand none\n",
            "    ProxyJump none\n",
            "    CanonicalizeHostname no\n",
            "    ControlMaster no\n",
            "    ControlPath none\n",
            "    ForwardAgent no\n",
            "    ForwardX11 no\n",
            "    ClearAllForwardings yes\n",
            "    PermitLocalCommand no\n",
            "    RequestTTY no\n",
            "    ConnectTimeout 10\n",
            "    RequiredRSASize 2048\n",
            "Host *\n",
            "    ProxyCommand /usr/bin/false\n",
        ),
        alias = HOST_ALIAS,
        hostname = address.host,
        port = address.port,
        user = address.user,
        algorithms = algorithms
    );
    write_private(&config, configuration.as_bytes())
        .await
        .map_err(|_| GitSourceError::SshConnection)?;
    command
        .env("MARSA_GIT_SSH_HOME", base)
        // Git appends its fixed SSH arguments to the shell's positional
        // parameters. Only this literal is evaluated as shell code.
        .env(
            "GIT_SSH_COMMAND",
            r#"cd "$MARSA_GIT_SSH_HOME" && exec /usr/bin/ssh -F ./config"#,
        )
        .env("GIT_SSH_VARIANT", "ssh")
        .args([
            "-c",
            "protocol.https.allow=never",
            "-c",
            "protocol.ssh.allow=always",
        ])
        .arg("-c")
        .arg(format!(
            "url.ssh://{HOST_ALIAS}/{}.insteadOf={url}",
            address.path
        ));
    Ok(Session {
        _identity: identity,
    })
}

struct Identity {
    _lease: ChildStdin,
    _supervisor: Child,
    directory: tempfile::TempDir,
    fingerprint: String,
}

impl Identity {
    async fn read(path: &Path) -> Result<Self, GitSourceError> {
        if !valid_identity_path(path) {
            return Err(GitSourceError::SshIdentity);
        }
        trusted_program(AGENT).await?;
        trusted_program(ADD).await?;
        let directory = super::private_git_directory()
            .await
            .map_err(|_| GitSourceError::SshIdentity)?;
        let workspace = directory.path();
        let mut supervisor = Command::new("/bin/sh")
            .env_clear()
            .current_dir(workspace)
            .args(["-c", AGENT_SUPERVISOR, "marsa-ssh-agent", LIFETIME])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| GitSourceError::SshIdentity)?;
        let lease = supervisor.stdin.take().ok_or(GitSourceError::SshIdentity)?;
        let fingerprint = timeout(Duration::from_secs(10), async {
            while !fs::symlink_metadata(workspace.join(SOCKET))
                .await
                .is_ok_and(|metadata| {
                    metadata.file_type().is_socket()
                        && metadata.uid() == nix::unistd::geteuid().as_raw()
                        && metadata.mode() & 0o077 == 0
                })
            {
                if supervisor.try_wait().ok()?.is_some() {
                    return None;
                }
                sleep(Duration::from_millis(25)).await;
            }

            let mut bytes = platform::read_user_docker_file(path, KEY_LIMIT, true)
                .await
                .ok()?;
            let loaded = load_identity(workspace, &bytes).await;
            bytes.fill(0);
            if !loaded? {
                return None;
            }

            let mut inspect = agent_command(workspace)
                .arg("-L")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .ok()?;
            let mut public = String::new();
            inspect
                .stdout
                .take()?
                .take((PUBLIC_KEY_LIMIT + 1) as u64)
                .read_to_string(&mut public)
                .await
                .ok()?;
            if !inspect.wait().await.ok()?.success() {
                return None;
            }
            let key = PublicKey::parse(public.trim_end())?;
            let public_key = format!("{} {}\n", key.kind, key.encoded);
            write_private(&workspace.join("identity.pub"), public_key.as_bytes())
                .await
                .ok()?;
            Some(key.fingerprint())
        })
        .await
        .ok()
        .flatten()
        .ok_or(GitSourceError::SshIdentity)?;
        Ok(Self {
            _lease: lease,
            _supervisor: supervisor,
            directory,
            fingerprint,
        })
    }
}

fn agent_command(workspace: &Path) -> Command {
    let mut command = Command::new(ADD);
    command
        .current_dir(workspace)
        .env_clear()
        .env("SSH_AUTH_SOCK", SOCKET)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn load_identity(workspace: &Path, bytes: &[u8]) -> Option<bool> {
    let mut child = agent_command(workspace)
        .args(["-q", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdin.take()?;
    pipe.write_all(bytes).await.ok()?;
    drop(pipe);
    Some(child.wait().await.ok()?.success())
}

fn valid_identity_path(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && !path.components().any(|part| part == Component::ParentDir)
        && path
            .to_str()
            .is_some_and(|name| name.len() <= 4096 && !name.chars().any(char::is_control))
}

async fn trusted_program(path: &str) -> Result<(), GitSourceError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|_| GitSourceError::SshConnection)?;
    if nix::unistd::geteuid().is_root()
        || !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o6022 != 0
    {
        return Err(GitSourceError::SshConnection);
    }
    Ok(())
}

async fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(contents).await?;
    file.flush().await
}

struct PublicKey<'a> {
    kind: &'a str,
    encoded: &'a str,
    bytes: Vec<u8>,
}

impl<'a> PublicKey<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        if value.len() > PUBLIC_KEY_LIMIT || value.chars().any(|ch| ch.is_control() && ch != '\t') {
            return None;
        }
        let mut parts = value.split_ascii_whitespace();
        let kind = parts.next()?;
        let encoded = parts.next()?;
        let bytes = STANDARD.decode(encoded).ok()?;
        let mut wire = bytes.as_slice();
        if ssh_string(&mut wire)? != kind.as_bytes() {
            return None;
        }
        match kind {
            "ssh-ed25519" if ssh_string(&mut wire)?.len() == 32 => {}
            "ssh-rsa"
                if !ssh_string(&mut wire)?.is_empty() && ssh_string(&mut wire)?.len() >= 256 => {}
            "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" => {
                let curve = kind.strip_prefix("ecdsa-sha2-")?;
                let expected = match curve {
                    "nistp256" => 65,
                    "nistp384" => 97,
                    _ => 133,
                };
                if ssh_string(&mut wire)? != curve.as_bytes() {
                    return None;
                }
                let point = ssh_string(&mut wire)?;
                if point.len() != expected || point.first() != Some(&4) {
                    return None;
                }
            }
            _ => return None,
        }
        wire.is_empty().then_some(Self {
            kind,
            encoded,
            bytes,
        })
    }

    fn fingerprint(&self) -> String {
        format!(
            "SHA256:{}",
            STANDARD_NO_PAD.encode(Sha256::digest(&self.bytes))
        )
    }
}

fn ssh_string<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (length, rest) = bytes.split_at_checked(4)?;
    let length = u32::from_be_bytes(length.try_into().ok()?) as usize;
    let (value, remaining) = rest.split_at_checked(length)?;
    *bytes = remaining;
    Some(value)
}
