//! Read registered Docker contexts and pin authenticated remote transport details.

use std::{
    fmt, io,
    net::Ipv6Addr,
    path::{Path, PathBuf},
    sync::Arc,
};

use hyper::http::uri::Authority;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{fs, net::TcpStream};
use tokio_rustls::{
    TlsConnector,
    rustls::{
        ClientConfig, RootCertStore,
        crypto::ring,
        pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject},
    },
};

use crate::platform;

use super::{DockerTransport, inventory::ExternalTransport};

const CONFIG_LIMIT: usize = 64 * 1024;
const PEM_LIMIT: usize = 128 * 1024;
const HOST_LIMIT: usize = 2048;
const MAX_CONTEXTS: usize = 1024;

#[derive(Deserialize)]
struct ContextMetadata {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Endpoints")]
    endpoints: ContextEndpoints,
}

#[derive(Deserialize)]
struct ContextEndpoints {
    docker: ContextDockerEndpoint,
}

#[derive(Deserialize)]
struct ContextDockerEndpoint {
    #[serde(rename = "Host")]
    host: String,
    #[serde(rename = "SkipTLSVerify", default)]
    skip_tls_verify: bool,
}

pub(super) struct RegisteredContext {
    pub(super) name: String,
    pub(super) host: String,
    tls_directory: Option<PathBuf>,
}

impl RegisteredContext {
    pub(super) async fn transport(&self) -> io::Result<ExternalTransport> {
        if let Some(endpoint) = self
            .host
            .strip_prefix("ssh://")
            .and_then(SshEndpoint::parse)
        {
            return Ok(ExternalTransport::Ssh(endpoint));
        }
        if let Some(address) = self.host.strip_prefix("tcp://")
            && let Some(directory) = &self.tls_directory
        {
            return Ok(ExternalTransport::Tls(
                TlsEndpoint::load(address, directory).await?,
            ));
        }
        Err(io::ErrorKind::Unsupported.into())
    }
}

pub(super) fn environment_context(host: String, config: &Path) -> Option<RegisteredContext> {
    if !valid_host(&host) {
        return None;
    }
    let tls_directory = std::env::var_os("DOCKER_TLS_VERIFY")
        .filter(|value| !value.is_empty())
        .map(|_| {
            std::env::var_os("DOCKER_CERT_PATH")
                .filter(|value| !value.is_empty())
                .map_or_else(|| config.to_path_buf(), PathBuf::from)
        });
    Some(RegisteredContext {
        name: "DOCKER_HOST".into(),
        host,
        tls_directory,
    })
}

/// Docker v29 stores context metadata and TLS files under the SHA-256 of the name.
/// Read this format directly so discovery never changes CLI selection or runs a plugin.
pub(super) async fn registered_contexts(config: &Path) -> Vec<RegisteredContext> {
    let contexts = config.join("contexts");
    let metadata = contexts.join("meta");
    for path in [config, &contexts, &metadata] {
        if platform::check_user_docker_directory(path).await.is_err() {
            return Vec::new();
        }
    }
    let Ok(mut entries) = fs::read_dir(metadata).await else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for _ in 0..MAX_CONTEXTS {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let slot = entry.file_name();
        let Some(slot) = slot
            .to_str()
            .filter(|slot| slot.len() == 64 && slot.bytes().all(|byte| byte.is_ascii_hexdigit()))
        else {
            continue;
        };
        if platform::check_user_docker_directory(&entry.path())
            .await
            .is_err()
        {
            continue;
        }
        let Ok(data) =
            platform::read_user_docker_file(&entry.path().join("meta.json"), CONFIG_LIMIT, false)
                .await
        else {
            continue;
        };
        let Ok(ContextMetadata { name, endpoints }) = serde_json::from_slice(&data) else {
            continue;
        };
        if name == "default"
            || !valid_context_name(&name)
            || context_id(&name) != slot
            || !valid_host(&endpoints.docker.host)
        {
            continue;
        }
        let tls = contexts.join("tls");
        let ctx_tls = tls.join(slot);
        let mut tls_directory = None;
        if endpoints.docker.host.starts_with("tcp://")
            && !endpoints.docker.skip_tls_verify
            && platform::check_user_docker_directory(&tls).await.is_ok()
            && platform::check_user_docker_directory(&ctx_tls)
                .await
                .is_ok()
        {
            tls_directory = Some(ctx_tls.join("docker"));
        }
        result.push(RegisteredContext {
            name,
            host: endpoints.docker.host,
            tls_directory,
        });
    }
    result.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    result
}

fn valid_context_name(name: &str) -> bool {
    (2..=255).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'+'))
}

fn context_id(name: &str) -> String {
    use std::fmt::Write as _;

    let mut id = String::with_capacity(64);
    for byte in Sha256::digest(name.as_bytes()) {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

fn valid_host(host: &str) -> bool {
    !host.is_empty() && host.len() <= HOST_LIMIT && !host.chars().any(char::is_control)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SshEndpoint {
    user: Option<String>,
    host: String,
    port: Option<u16>,
    socket: Option<String>,
    pinned: Option<SshTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SshTarget {
    hostname: String,
    user: String,
    port: u16,
    host_key_alias: Option<String>,
}

impl SshTarget {
    fn parse(config: &str) -> Option<Self> {
        let field = |name| {
            config
                .lines()
                .filter_map(|line| line.split_once(' '))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value)
        };
        let hostname = field("hostname")?;
        let user = field("user")?;
        let port = field("port")?
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)?;
        let host_key_alias = field("hostkeyalias").map(str::to_owned);
        if !(valid_ssh_component(hostname) || hostname.parse::<Ipv6Addr>().is_ok())
            || !user.split('@').all(valid_ssh_component)
            || host_key_alias
                .as_ref()
                .is_some_and(|alias| !valid_host(alias))
        {
            return None;
        }
        Some(Self {
            hostname: hostname.to_owned(),
            user: user.to_owned(),
            port,
            host_key_alias,
        })
    }
}

impl SshEndpoint {
    fn parse(value: &str) -> Option<Self> {
        if value.contains(['?', '#']) {
            return None;
        }
        let (address, path) = value.split_once('/').unwrap_or((value, ""));
        let (user, authority) = match address.split_once('@') {
            Some((user, authority)) if valid_ssh_component(user) => {
                (Some(user.to_owned()), authority)
            }
            Some(_) => return None,
            None => (None, address),
        };
        let authority: Authority = authority.parse().ok()?;
        let (host, port) = host_and_port(&authority)?;
        let socket = if path.trim_matches('/').is_empty() {
            None
        } else {
            Some(format!("/{}", decode_ssh_path(path)?))
        };
        Some(Self {
            user,
            host: host.to_owned(),
            port,
            socket,
            pinned: None,
        })
    }

    pub(super) async fn pin(&mut self) {
        self.pinned = Self::resolve(&self.arguments()).await.ok();
    }

    pub(super) async fn connect(&self) -> io::Result<DockerTransport> {
        let pinned = self.pinned.as_ref().ok_or(io::ErrorKind::NotConnected)?;
        let args = self.arguments();
        // Re-evaluate the user's SSH config before dialing. The explicit host,
        // user and port also pin the connection's destination during startup.
        if Self::resolve(&args).await? != *pinned {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(Box::new(platform::open_docker_ssh(&args)?))
    }

    async fn resolve(args: &[String]) -> io::Result<SshTarget> {
        let config = platform::docker_ssh_config(args).await?;
        SshTarget::parse(&config).ok_or_else(|| io::ErrorKind::InvalidData.into())
    }

    pub(super) fn address(&self) -> String {
        let user = self
            .user
            .as_ref()
            .map_or(String::new(), |user| format!("{user}@"));
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let port = self.port.map_or(String::new(), |port| format!(":{port}"));
        let socket = self.socket.as_deref().unwrap_or_default();
        format!("ssh://{user}{host}{port}{socket}")
    }

    fn arguments(&self) -> Vec<String> {
        let mut args = [
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "UpdateHostKeys=no",
            "-o",
            "VerifyHostKeyDNS=no",
            "-o",
            "ControlPath=none",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ForwardAgent=no",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "ConnectTimeout=8",
        ]
        .map(str::to_owned)
        .to_vec();
        if let Some(pinned) = &self.pinned {
            args.extend(["-o".to_owned(), format!("HostName={}", pinned.hostname)]);
            if let Some(alias) = &pinned.host_key_alias {
                args.extend(["-o".to_owned(), format!("HostKeyAlias={alias}")]);
            }
        }
        if let Some(user) = self
            .pinned
            .as_ref()
            .map(|target| &target.user)
            .or(self.user.as_ref())
        {
            args.extend(["-l".to_owned(), user.clone()]);
        }
        if let Some(port) = self.pinned.as_ref().map(|target| target.port).or(self.port) {
            args.extend(["-p".to_owned(), port.to_string()]);
        }
        args.extend(["--".to_owned(), self.host.clone()]);
        // OpenSSH passes the remote command through the login shell. Single-quote
        // the Unix path and escape embedded quotes before sending it as one argument.
        let command = match &self.socket {
            Some(socket) => format!(
                "docker --host='unix://{}' system dial-stdio",
                socket.replace('\'', "'\\''")
            ),
            None => "docker system dial-stdio".to_owned(),
        };
        args.push(command);
        args
    }
}

fn valid_ssh_component(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'+'))
}

fn host_and_port(address: &Authority) -> Option<(&str, Option<u16>)> {
    let host = address.host();
    let host = match host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        Some(ip) if ip.parse::<Ipv6Addr>().is_ok() => ip,
        Some(_) => return None,
        None if valid_ssh_component(host) => host,
        None => return None,
    };
    let port = address.port().map(|port| port.as_u16());
    // The Authority type accepts userinfo and parses overlarge ports as absent.
    // Only the host substring and a valid numeric port are allowed here.
    if address.as_str().contains('@')
        || (address.as_str().contains(':') && port.is_none() && !address.as_str().ends_with(']'))
        || port == Some(0)
    {
        return None;
    }
    Some((host, port))
}

fn decode_ssh_path(path: &str) -> Option<String> {
    let mut bytes = path.bytes();
    let mut output = Vec::with_capacity(path.len());
    while let Some(byte) = bytes.next() {
        let value = if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)? as u8;
            let low = (bytes.next()? as char).to_digit(16)? as u8;
            (high << 4) | low
        } else {
            byte
        };
        if value.is_ascii_control() {
            return None;
        }
        output.push(value);
    }
    String::from_utf8(output).ok()
}

#[derive(Clone)]
pub(super) struct TlsEndpoint {
    authority: Authority,
    server: ServerName<'static>,
    config: Arc<ClientConfig>,
    identity: [u8; 32],
}

impl fmt::Debug for TlsEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TlsEndpoint")
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TlsEndpoint {
    fn eq(&self, other: &Self) -> bool {
        self.authority == other.authority && self.identity == other.identity
    }
}

impl Eq for TlsEndpoint {}

impl TlsEndpoint {
    async fn load(address: &str, directory: &Path) -> io::Result<Self> {
        let parsed: Authority = address.parse().map_err(io::Error::other)?;
        let (hostname, port) = host_and_port(&parsed).ok_or(io::ErrorKind::InvalidInput)?;
        let server = ServerName::try_from(hostname.to_owned()).map_err(io::Error::other)?;
        let authority = if port.is_some() {
            parsed
        } else {
            format!("{parsed}:2376").parse().map_err(io::Error::other)?
        };
        platform::check_user_docker_directory(directory).await?;
        let (root, cert, key) = tokio::try_join!(
            read_pem(directory.join("ca.pem"), false),
            read_pem(directory.join("cert.pem"), false),
            read_pem(directory.join("key.pem"), true),
        )?;
        let mut roots = RootCertStore::empty();
        for cert in CertificateDer::pem_slice_iter(&root) {
            roots
                .add(cert.map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        }
        if roots.is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let chain: Vec<_> = CertificateDer::pem_slice_iter(&cert)
            .collect::<Result<_, _>>()
            .map_err(io::Error::other)?;
        let private = PrivateKeyDer::from_pem_slice(&key).map_err(io::Error::other)?;
        let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_root_certificates(roots)
            .with_client_auth_cert(chain, private)
            .map_err(io::Error::other)?;
        let identity = Sha256::new()
            .chain_update(Sha256::digest(root))
            .chain_update(Sha256::digest(cert))
            .finalize()
            .into();
        Ok(Self {
            authority,
            server,
            config: Arc::new(config),
            identity,
        })
    }

    pub(super) fn authority(&self) -> &str {
        self.authority.as_str()
    }

    pub(super) async fn connect(&self) -> io::Result<DockerTransport> {
        let socket = TcpStream::connect(self.authority.as_str()).await?;
        let stream = TlsConnector::from(self.config.clone())
            .connect(self.server.clone(), socket)
            .await?;
        Ok(Box::new(stream))
    }
}

async fn read_pem(path: PathBuf, private: bool) -> io::Result<Vec<u8>> {
    platform::read_user_docker_file(&path, PEM_LIMIT, private).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_requires_verified_host_keys_and_uses_docker_dial_stdio() {
        let spec = SshEndpoint::parse("user@docker.example:2222/run/docker.sock").unwrap();
        let args = spec.arguments();
        assert!(args.iter().any(|arg| arg == "StrictHostKeyChecking=yes"));
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
        assert!(args.iter().any(|arg| arg == "ControlPath=none"));
        assert!(args.ends_with(&[
            "--".to_owned(),
            "docker.example".to_owned(),
            "docker --host='unix:///run/docker.sock' system dial-stdio".to_owned(),
        ]));
        assert!(SshEndpoint::parse("user:password@docker.example").is_none());
        assert!(SshEndpoint::parse("-oProxyCommand=anything").is_none());
        assert!(SshEndpoint::parse("docker.example?option=yes").is_none());
        assert!(SshEndpoint::parse("docker.example/%00").is_none());
        assert!(SshEndpoint::parse("user@[::1]:2222").is_some());
    }

    #[test]
    fn ssh_context_pins_the_resolved_target_while_retaining_the_alias() {
        let mut endpoint = SshEndpoint::parse("build-host").unwrap();
        endpoint.pinned = SshTarget::parse(
            "hostname docker.example\nuser builder\nport 2022\nhostkeyalias docker-team\n",
        );
        let args = endpoint.arguments();
        assert!(args.iter().any(|arg| arg == "HostName=docker.example"));
        assert!(args.iter().any(|arg| arg == "HostKeyAlias=docker-team"));
        assert!(args.windows(2).any(|pair| pair == ["-l", "builder"]));
        assert!(args.windows(2).any(|pair| pair == ["-p", "2022"]));
        assert!(args.windows(2).any(|pair| pair == ["--", "build-host"]));
        assert!(SshTarget::parse("hostname localhost\nuser builder\nport 0\n").is_none());
    }

    #[tokio::test]
    async fn discovery_reads_only_named_docker_contexts_from_the_pinned_layout() {
        let config = tempfile::tempdir().unwrap();
        for (slot, name, host) in [
            (context_id("office"), "office", "ssh://myserver"),
            (context_id("desktop"), "desktop", "unix:///tmp/desktop.sock"),
            (context_id("wrong"), "mismatched", "unix:///tmp/bogus.sock"),
        ] {
            let path = config.path().join("contexts/meta").join(slot);
            tokio::fs::create_dir_all(&path).await.unwrap();
            let metadata =
                serde_json::json!({"Name": name, "Endpoints": {"docker": {"Host": host}}});
            tokio::fs::write(path.join("meta.json"), metadata.to_string())
                .await
                .unwrap();
        }
        let contexts = registered_contexts(config.path()).await;
        assert_eq!(
            contexts
                .iter()
                .map(|context| context.name.as_str())
                .collect::<Vec<_>>(),
            ["desktop", "office"]
        );
    }

    #[tokio::test]
    async fn tcp_needs_verified_context_tls_material() {
        let scratch = tempfile::tempdir().unwrap();
        assert!(
            TlsEndpoint::load("docker.example:2376", scratch.path())
                .await
                .is_err()
        );
        for name in ["ca.pem", "cert.pem", "key.pem"] {
            tokio::fs::write(scratch.path().join(name), "garbage")
                .await
                .unwrap();
        }
        assert!(
            TlsEndpoint::load("docker.example:2376", scratch.path())
                .await
                .is_err()
        );
    }
}
