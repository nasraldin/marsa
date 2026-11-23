//! Discover and inspect external Docker engines without changing the active context.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use hyper::Method;
use tokio::task::JoinSet;

use super::{
    ContainerAction, ContainerDetails, ContainerId, ContainerSummary, DockerClient, DockerError,
    DockerLogs, DockerScope, DockerStats, DockerTransport, ExecSession, ImageSummary,
    ManagedEndpointError, ManagedEndpointStore, NetworkSummary, ResourceName, TerminalSize,
    VerifiedEngine, VolumeSummary,
    contexts::{self, SshEndpoint, TlsEndpoint},
    docker_socket_path, managed, transfer,
};
use crate::platform;

/// Authenticated endpoint captured by discovery. Operations never consult the current context again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEndpoint {
    context: Option<String>,
    transport: ExternalTransport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExternalTransport {
    Unix {
        socket: PathBuf,
        identity: Option<platform::SocketIdentity>,
    },
    Ssh(SshEndpoint),
    Tls(TlsEndpoint),
    Unavailable,
}

impl ExternalEndpoint {
    pub(super) fn managed(id: String, transport: ExternalTransport) -> Self {
        Self {
            context: Some(id),
            transport,
        }
    }

    /// Return the stable source ID of a Marsa-managed endpoint.
    pub fn managed_id(&self) -> Option<&str> {
        self.context
            .as_deref()
            .filter(|id| managed::is_managed_id(id))
    }

    pub(super) async fn pin_ssh(&mut self) {
        if let ExternalTransport::Ssh(ssh) = &mut self.transport {
            ssh.pin().await;
        }
    }

    pub(super) async fn verify_engine(&self) -> Result<VerifiedEngine, ManagedEndpointError> {
        let engine: VerifiedEngine =
            self.client(false)
                .snapshot("/info")
                .await
                .map_err(|error| match error {
                    DockerError::Json(_) => ManagedEndpointError::InvalidEngineIdentity,
                    other => other.into(),
                })?;
        if !engine.valid() {
            return Err(ManagedEndpointError::InvalidEngineIdentity);
        }
        Ok(engine)
    }

    fn client(&self, containers: bool) -> DockerClient {
        DockerClient {
            scope: DockerScope::External {
                endpoint: self.clone(),
                containers,
            },
        }
    }

    /// Return the canonical path of a Unix socket endpoint, if applicable.
    pub fn socket(&self) -> Option<&Path> {
        match &self.transport {
            ExternalTransport::Unix { socket, .. } => Some(socket),
            _ => None,
        }
    }

    /// Return the endpoint's address without displaying credentials.
    pub fn address(&self) -> String {
        match &self.transport {
            ExternalTransport::Unix { socket, .. } => socket.to_string_lossy().into_owned(),
            ExternalTransport::Ssh(endpoint) => endpoint.address(),
            ExternalTransport::Tls(endpoint) => format!("tcp://{}", endpoint.authority()),
            ExternalTransport::Unavailable => String::new(),
        }
    }

    pub(super) fn request_host(&self) -> &str {
        match &self.transport {
            ExternalTransport::Tls(endpoint) => endpoint.authority(),
            _ => "docker",
        }
    }

    /// Open a transport using captured authentication details for the typed client and Compose.
    pub(crate) async fn open_transport(&self) -> io::Result<DockerTransport> {
        if nix::unistd::geteuid().is_root() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        match &self.transport {
            ExternalTransport::Unix {
                socket,
                identity: Some(identity),
            } => Ok(Box::new(
                platform::open_external_socket(socket.clone(), *identity).await?,
            )),
            ExternalTransport::Ssh(endpoint) => endpoint.connect().await,
            ExternalTransport::Tls(endpoint) => endpoint.connect().await,
            _ => Err(io::ErrorKind::NotConnected.into()),
        }
    }

    /// Read details from this engine and verify the returned full container identifier.
    pub async fn inspect(&self, id: &ContainerId) -> Result<ContainerDetails, DockerError> {
        self.client(true).inspect(id).await
    }

    /// Stream the selected container's bounded log output from this engine.
    pub async fn logs(&self, id: &ContainerId) -> Result<DockerLogs, DockerError> {
        self.client(true).logs(id).await
    }

    /// Follow live container resource usage on the discovered engine.
    pub async fn stats(&self, id: &ContainerId) -> Result<DockerStats, DockerError> {
        self.client(true).stats(id).await
    }

    /// Open a bounded interactive shell on the discovered engine.
    pub async fn start_terminal(
        &self,
        id: &ContainerId,
        size: TerminalSize,
    ) -> Result<ExecSession, DockerError> {
        self.client(true).start_terminal(id, size).await
    }

    /// Start, stop, or restart the selected container on this engine.
    pub async fn act(&self, id: &ContainerId, action: ContainerAction) -> Result<(), DockerError> {
        self.client(true).act(id, action).await
    }

    /// Open this engine's typed management client. Every request is pinned to discovery.
    pub fn management_client(&self) -> DockerClient {
        self.client(true)
    }

    /// Refresh this engine's container snapshot without searching for another endpoint.
    pub async fn containers(&self) -> Result<Vec<ContainerSummary>, DockerError> {
        self.client(false).containers().await
    }
}

pub(super) fn external_route(
    method: &Method,
    path: &str,
    containers: bool,
    has_body: bool,
) -> bool {
    if !has_body
        && method == Method::GET
        && matches!(
            path,
            "/containers/json?all=true" | "/images/json?all=true" | "/volumes" | "/networks"
        )
    {
        return true;
    }
    if !containers {
        return false;
    }
    if resource_route(method, path, has_body)
        || transfer::external_transfer_route(method, path, has_body)
    {
        return true;
    }
    if let Some((id, action)) = path
        .strip_prefix("/exec/")
        .and_then(|tail| tail.split_once('/'))
    {
        return method == Method::POST
            && valid_id(id)
            && match action {
                "start" => has_body,
                query if !has_body => query
                    .strip_prefix("resize?h=")
                    .and_then(|size| size.split_once("&w="))
                    .is_some_and(|(rows, columns)| {
                        [rows, columns].into_iter().all(|value| {
                            value.parse::<u16>().is_ok_and(|n| (1..=1024).contains(&n))
                        })
                    }),
                _ => false,
            };
    }
    let Some((id, action)) = path
        .strip_prefix("/containers/")
        .and_then(|tail| tail.split_once('/'))
    else {
        return false;
    };
    if !valid_id(id) {
        return false;
    }
    match *method {
        Method::GET if !has_body => {
            matches!(action, "json" | super::LOG_ENDPOINT | "stats?stream=true")
        }
        Method::POST if has_body => action == "exec",
        Method::POST => matches!(action, "start" | "stop" | "restart" | "pause" | "unpause"),
        _ => false,
    }
}

fn resource_route(method: &Method, path: &str, has_body: bool) -> bool {
    match *method {
        Method::POST if has_body => {
            matches!(
                path,
                "/containers/create" | "/volumes/create" | "/networks/create"
            ) || path
                .strip_prefix("/containers/create?name=")
                .is_some_and(|name| ResourceName::parse(name).is_ok())
        }
        Method::POST => {
            path.strip_prefix("/images/create?fromImage=")
                .is_some_and(valid_pull_query)
                || path
                    .strip_prefix("/images/sha256:")
                    .and_then(|path| path.split_once("/tag?repo="))
                    .is_some_and(|(digest, query)| {
                        valid_id(digest)
                            && query.split_once("&tag=").is_some_and(|(repo, tag)| {
                                valid_pull_query(repo) && super::images::valid_tag(tag)
                            })
                    })
        }
        Method::GET if !has_body => {
            path == "/system/df?type=volume&verbose=true"
                || path
                    .strip_prefix("/images/sha256:")
                    .and_then(|digest| digest.strip_suffix("/json"))
                    .is_some_and(valid_id)
        }
        Method::DELETE if !has_body => {
            path.strip_prefix("/images/sha256:")
                .and_then(with_force)
                .is_some_and(valid_id)
                || path.strip_prefix("/networks/").is_some_and(valid_id)
                || path
                    .strip_prefix("/volumes/")
                    .and_then(|name| name.strip_suffix("?force=false"))
                    .is_some_and(|name| ResourceName::parse(name).is_ok())
                || path
                    .strip_prefix("/containers/")
                    .and_then(with_force)
                    .is_some_and(valid_id)
        }
        _ => false,
    }
}

fn with_force(path: &str) -> Option<&str> {
    path.strip_suffix("?force=false")
        .or_else(|| path.strip_suffix("?force=true"))
}

fn valid_pull_query(reference: &str) -> bool {
    let mut bytes = reference.bytes();
    let mut length = 0;
    while let Some(byte) = bytes.next() {
        length += 1;
        if byte == b'%' {
            if !matches!(
                (bytes.next(), bytes.next()),
                (Some(b'3'), Some(b'A'))
                    | (Some(b'2'), Some(b'F'))
                    | (Some(b'4'), Some(b'0'))
                    | (Some(b'5'), Some(b'B' | b'D'))
            ) {
                return false;
            }
        } else if !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'_' | b'-') {
            return false;
        }
    }
    (1..=512).contains(&length)
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Current resources and reachability of an external Docker engine.
pub struct ExternalInventory {
    /// Pinned engine connection for the detected resources.
    pub endpoint: ExternalEndpoint,
    /// Display label of the managed configuration, Docker context, or local provider.
    pub source: String,
    /// Whether a managed engine returned its identity or discovery read available inventory.
    pub reachable: bool,
    /// The engine identity verified for a managed endpoint, when available.
    pub verified_engine: Option<VerifiedEngine>,
    /// Running and stopped containers.
    pub containers: Vec<ContainerSummary>,
    /// Tagged and untagged images.
    pub images: Vec<ImageSummary>,
    /// Volumes when the endpoint reports them.
    pub volumes: Option<Vec<VolumeSummary>>,
    /// Networks when the endpoint reports them.
    pub networks: Option<Vec<NetworkSummary>>,
}

/// Read the inventory of the first reachable external engine.
pub async fn discover_external_inventory() -> Option<ExternalInventory> {
    discover_external_inventories()
        .await
        .into_iter()
        .find(|entry| entry.reachable)
}

/// Read inventories for managed endpoints, Docker contexts and reachable local providers.
/// Managed endpoints and registered contexts remain visible when unavailable.
pub async fn discover_external_inventories() -> Vec<ExternalInventory> {
    if nix::unistd::geteuid().is_root() {
        return Vec::new();
    }
    let Some(owner) = tokio::task::spawn_blocking(|| {
        nix::unistd::User::from_uid(nix::unistd::geteuid())
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten() else {
        return Vec::new();
    };
    let config = std::env::var_os("DOCKER_CONFIG")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            std::env::home_dir()
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| owner.dir.clone())
                .join(".docker")
        });
    let config = tokio::fs::canonicalize(&config).await.unwrap_or(config);
    let explicit = std::env::var("DOCKER_HOST")
        .ok()
        .and_then(|host| contexts::environment_context(host, &config));
    let registered = contexts::registered_contexts(&config).await;
    let mut seen = HashSet::new();
    let default_marsa = owner.dir.join(".marsa/run/docker.sock");
    for path in docker_socket_path().ok().into_iter().chain([default_marsa]) {
        seen.insert(tokio::fs::canonicalize(&path).await.unwrap_or(path));
    }
    let mut entries = Vec::new();
    for (context, environment) in registered
        .iter()
        .map(|context| (context, false))
        .chain(explicit.iter().map(|context| (context, true)))
    {
        let transport = if let Some(path) = unix_host(&context.host) {
            let socket = tokio::fs::canonicalize(&path).await.unwrap_or(path);
            if !seen.insert(socket.clone()) {
                continue;
            }
            let identity = platform::socket_identity(&socket).await.ok();
            ExternalTransport::Unix { socket, identity }
        } else {
            context
                .transport()
                .await
                .unwrap_or(ExternalTransport::Unavailable)
        };
        let source = match &transport {
            ExternalTransport::Unix { socket, .. } if environment => {
                source_name(socket, &owner.dir)
            }
            _ => context.name.clone(),
        };
        entries.push((
            source,
            ExternalEndpoint {
                context: Some(context.name.clone()),
                transport,
            },
            true,
        ));
    }
    for candidate in provider_candidates(&owner.dir).await {
        let Ok(socket) = tokio::fs::canonicalize(candidate).await else {
            continue;
        };
        if !seen.insert(socket.clone()) {
            continue;
        }
        let Ok(identity) = platform::socket_identity(&socket).await else {
            continue;
        };
        let source = source_name(&socket, &owner.dir);
        let transport = ExternalTransport::Unix {
            socket,
            identity: Some(identity),
        };
        entries.push((
            source,
            ExternalEndpoint {
                context: None,
                transport,
            },
            false,
        ));
    }
    if let Ok(store) = ManagedEndpointStore::discover()
        && let Ok(managed) = store.list().await
    {
        for endpoint in managed {
            let transport = endpoint
                .config
                .capture_transport()
                .await
                .unwrap_or(ExternalTransport::Unavailable);
            entries.push((
                endpoint.config.name,
                ExternalEndpoint::managed(endpoint.id, transport),
                true,
            ));
        }
    }
    probe_endpoints(entries).await
}

async fn probe_endpoints(entries: Vec<(String, ExternalEndpoint, bool)>) -> Vec<ExternalInventory> {
    const MAX_PARALLEL_ENGINES: usize = 8;
    let mut pending = entries.into_iter().enumerate();
    let mut probes = JoinSet::new();
    let mut results = Vec::new();
    loop {
        while probes.len() < MAX_PARALLEL_ENGINES {
            let Some((index, (source, endpoint, show_offline))) = pending.next() else {
                break;
            };
            probes.spawn(async move {
                let inventory = probe_engine(source, endpoint).await;
                (
                    index,
                    (inventory.reachable || show_offline).then_some(inventory),
                )
            });
        }
        let Some(result) = probes.join_next().await else {
            break;
        };
        if let Ok((index, Some(inventory))) = result {
            results.push((index, inventory));
        }
    }
    results.sort_unstable_by_key(|(index, _)| *index);
    results
        .into_iter()
        .map(|(_, inventory)| inventory)
        .collect()
}

async fn probe_engine(source: String, mut endpoint: ExternalEndpoint) -> ExternalInventory {
    endpoint.pin_ssh().await;
    let verified_engine = if endpoint.managed_id().is_some() {
        match endpoint.verify_engine().await {
            Ok(engine) => Some(engine),
            Err(_) => {
                return ExternalInventory {
                    endpoint,
                    source,
                    reachable: false,
                    verified_engine: None,
                    containers: Vec::new(),
                    images: Vec::new(),
                    volumes: None,
                    networks: None,
                };
            }
        }
    } else {
        None
    };
    let client = endpoint.client(false);
    let (containers, images, volumes, networks) = tokio::join!(
        client.containers(),
        client.images(),
        client.volumes(),
        client.networks(),
    );
    let reachable = verified_engine.is_some()
        || containers.is_ok()
        || images.is_ok()
        || volumes.is_ok()
        || networks.is_ok();
    ExternalInventory {
        endpoint,
        source,
        reachable,
        verified_engine,
        containers: containers.unwrap_or_default(),
        images: images.unwrap_or_default(),
        volumes: volumes.ok(),
        networks: networks.ok(),
    }
}

async fn provider_candidates(home: &Path) -> Vec<PathBuf> {
    let mut result: Vec<_> = candidates(home).into();
    for (provider, socket) in [(".colima", "docker.sock"), (".lima", "sock/docker.sock")] {
        let Ok(mut instances) = tokio::fs::read_dir(home.join(provider)).await else {
            continue;
        };
        for _ in 0..1024 {
            let Ok(Some(instance)) = instances.next_entry().await else {
                break;
            };
            result.push(instance.path().join(socket));
        }
    }
    result.sort();
    result
}

fn unix_host(host: &str) -> Option<PathBuf> {
    host.strip_prefix("unix://")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn source_name(socket: &Path, home: &Path) -> String {
    for (directory, provider) in [(".lima", "Lima"), (".colima", "Colima")] {
        if let Ok(relative) = socket.strip_prefix(home.join(directory))
            && let Some(instance) = relative.components().next()
        {
            return format!("{provider} · {}", instance.as_os_str().to_string_lossy());
        }
    }
    if socket.starts_with(home.join(".docker/run")) {
        return "Docker Desktop".into();
    }
    if socket.starts_with(home.join(".orbstack")) {
        return "OrbStack".into();
    }
    socket.to_string_lossy().into_owned()
}

fn candidates(home: &Path) -> [PathBuf; 5] {
    [
        home.join(".docker/run/docker.sock"),
        home.join(".orbstack/run/docker.sock"),
        home.join(".colima/default/docker.sock"),
        home.join(".lima/docker/sock/docker.sock"),
        PathBuf::from("/var/run/docker.sock"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inventory_uses_only_read_routes_on_a_user_owned_provider_socket() {
        use std::{fs, os::unix::fs::PermissionsExt};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::UnixListener,
        };

        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750)).unwrap();
        let socket = directory.path().join("docker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();

        let identity = platform::socket_identity(&socket).await.unwrap();
        let client = ExternalEndpoint {
            context: None,
            transport: ExternalTransport::Unix {
                socket,
                identity: Some(identity),
            },
        }
        .client(false);
        assert!(
            client
                .request(hyper::Method::POST, "/containers/example/start")
                .await
                .is_err()
        );
        assert!(client.request(hyper::Method::GET, "/info").await.is_err());

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            assert!(request.starts_with(b"GET /containers/json?all=true HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n[]")
                .await
                .unwrap();
        });
        assert!(client.containers().await.unwrap().is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn lifecycle_uses_the_discovered_socket_and_rejects_its_replacement() {
        use std::{fs, os::unix::fs::PermissionsExt};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::UnixListener,
        };

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("docker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let endpoint = ExternalEndpoint {
            context: None,
            transport: ExternalTransport::Unix {
                identity: Some(platform::socket_identity(&socket).await.unwrap()),
                socket: socket.clone(),
            },
        };
        let id = ContainerId::parse("a".repeat(64)).unwrap();
        let server_id = id.clone();
        let server = tokio::spawn(async move {
            for action in ["start", "stop"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                assert!(
                    request.starts_with(
                        format!(
                            "POST /containers/{}/{action} HTTP/1.1\r\n",
                            server_id.as_str()
                        )
                        .as_bytes()
                    )
                );
                stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
            listener
        });
        assert!(
            endpoint
                .client(false)
                .act(&id, ContainerAction::Restart)
                .await
                .is_err()
        );
        endpoint.act(&id, ContainerAction::Start).await.unwrap();
        endpoint.act(&id, ContainerAction::Stop).await.unwrap();
        let _original = server.await.unwrap();

        fs::rename(&socket, directory.path().join("original.sock")).unwrap();
        let _replacement = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            endpoint.act(&id, ContainerAction::Start).await,
            Err(DockerError::Socket(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn external_terminal_and_stats_accept_only_the_typed_routes() {
        let id = "a".repeat(64);
        let stats = format!("/containers/{id}/stats?stream=true");
        let create = format!("/containers/{id}/exec");
        let start = format!("/exec/{id}/start");
        let resize = format!("/exec/{id}/resize?h=24&w=80");
        assert!(external_route(&Method::GET, &stats, true, false));
        assert!(external_route(&Method::POST, &create, true, true));
        assert!(external_route(&Method::POST, &start, true, true));
        assert!(external_route(&Method::POST, &resize, true, false));
        assert!(!external_route(&Method::POST, &start, false, true));
        assert!(!external_route(&Method::GET, &stats, true, true));
        assert!(!external_route(&Method::POST, &create, true, false));
        assert!(!external_route(
            &Method::POST,
            "/exec/short/start",
            true,
            true
        ));
        assert!(!external_route(
            &Method::POST,
            &format!("{resize}&force=1"),
            true,
            false
        ));
    }

    #[test]
    fn external_resource_routes_require_supported_operations_and_valid_identifiers() {
        let digest = "b".repeat(64);
        for (method, path, has_body) in [
            (Method::GET, format!("/images/sha256:{digest}/json"), false),
            (
                Method::POST,
                format!("/images/sha256:{digest}/tag?repo=registry%3A5000%2Ftest&tag=dev"),
                false,
            ),
            (
                Method::DELETE,
                format!("/images/sha256:{digest}?force=false"),
                false,
            ),
            (Method::POST, "/volumes/create".into(), true),
            (
                Method::DELETE,
                "/volumes/database?force=false".into(),
                false,
            ),
            (Method::DELETE, format!("/networks/{digest}"), false),
        ] {
            assert!(
                external_route(&method, &path, true, has_body),
                "{method} {path}"
            );
            assert!(!external_route(&method, &path, false, has_body));
        }
        for path in [
            "/volumes/database?force=true",
            "/volumes/database%2Fother?force=false",
            "/images/alpine?force=false",
            "/networks/prune",
        ] {
            assert!(!external_route(&Method::DELETE, path, true, false));
        }
        assert!(!external_route(
            &Method::POST,
            "/images/create?fromImage=test&platform=windows",
            true,
            false
        ));
    }

    #[test]
    fn a_provider_badge_identifies_the_discovered_socket_instance() {
        let home = Path::new("/home/test");
        assert_eq!(
            source_name(&home.join(".lima/docker/sock/docker.sock"), home),
            "Lima · docker"
        );
        assert_eq!(
            source_name(Path::new("/custom/docker.sock"), home),
            "/custom/docker.sock"
        );
    }

    #[tokio::test]
    async fn inventory_retains_reachable_engines_and_offline_registered_contexts() {
        use std::{fs, os::unix::fs::PermissionsExt};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::UnixListener,
        };

        let directory = tempfile::tempdir().unwrap();
        let mut endpoints = Vec::new();
        let mut servers = Vec::new();
        for label in ["one", "two"] {
            let socket = directory.path().join(format!("{label}.sock"));
            let listener = UnixListener::bind(&socket).unwrap();
            fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
            endpoints.push((
                label.into(),
                ExternalEndpoint {
                    context: Some(label.into()),
                    transport: ExternalTransport::Unix {
                        identity: Some(platform::socket_identity(&socket).await.unwrap()),
                        socket,
                    },
                },
                true,
            ));
            servers.push(tokio::spawn(async move {
                for _ in 0..4 {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(stream.read_u8().await.unwrap());
                    }
                    assert!(request.starts_with(b"GET "));
                    let body = if request.starts_with(b"GET /volumes ") {
                        "{\"Volumes\":[] }"
                    } else {
                        "[]"
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            }));
        }
        endpoints.push((
            "offline".into(),
            ExternalEndpoint {
                context: Some("offline".into()),
                transport: ExternalTransport::Unavailable,
            },
            true,
        ));
        let results = probe_endpoints(endpoints).await;
        for server in servers {
            server.await.unwrap();
        }
        assert_eq!(
            results
                .iter()
                .map(|entry| (entry.source.as_str(), entry.reachable))
                .collect::<Vec<_>>(),
            [("one", true), ("two", true), ("offline", false)]
        );
        assert_ne!(results[0].endpoint, results[1].endpoint);
        assert_eq!(results[0].volumes.as_ref().map(Vec::len), Some(0));
    }

    #[test]
    fn configured_unix_hosts_require_absolute_paths() {
        assert_eq!(
            unix_host("unix:///test/provider.sock"),
            Some(PathBuf::from("/test/provider.sock"))
        );
        for host in ["tcp://localhost:2375", "ssh://remote", "unix://relative"] {
            assert_eq!(unix_host(host), None);
        }
    }
}
