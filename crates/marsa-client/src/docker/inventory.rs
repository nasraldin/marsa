//! Inspect a separate user-owned Docker engine without changing Marsa's endpoint.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use hyper::Method;

use super::{
    ContainerAction, ContainerDetails, ContainerId, ContainerSummary, DockerClient, DockerError,
    DockerLogs, DockerScope, DockerStats, ExecSession, ImageSummary, NetworkSummary, ResourceName,
    TerminalSize, VolumeSummary, docker_socket_path,
};
use crate::platform;

/// Authenticated endpoint captured by discovery. Operations never consult the current context again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEndpoint {
    socket: PathBuf,
    identity: platform::SocketIdentity,
}

impl ExternalEndpoint {
    fn client(&self, containers: bool) -> DockerClient {
        DockerClient {
            socket: self.socket.clone(),
            scope: DockerScope::External {
                identity: self.identity,
                containers,
            },
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
    if resource_route(method, path, has_body) {
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
        Method::GET if !has_body => matches!(
            action,
            "json" | "logs?stdout=true&stderr=true&follow=true&tail=200" | "stats?stream=true"
        ),
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

/// Containers and images reported by a detected local Docker engine.
pub struct ExternalInventory {
    /// Authenticated local socket queried for this inventory.
    pub socket: PathBuf,
    /// Pinned engine connection for the detected resources.
    pub endpoint: ExternalEndpoint,
    /// Provider or instance label for the socket.
    pub source: String,
    /// Running and stopped containers.
    pub containers: Vec<ContainerSummary>,
    /// Tagged and untagged images.
    pub images: Vec<ImageSummary>,
    /// Volumes when the endpoint reports them.
    pub volumes: Option<Vec<VolumeSummary>>,
    /// Networks when the endpoint reports them.
    pub networks: Option<Vec<NetworkSummary>>,
}

/// Read the inventory of the first reachable user-owned external Unix socket.
///
/// Prefer an explicit Unix DOCKER_HOST, then well-known local provider sockets.
/// An unreachable or foreign-owned socket is skipped without launching anything.
pub async fn discover_external_inventory() -> Option<ExternalInventory> {
    let owner = tokio::task::spawn_blocking(|| {
        nix::unistd::User::from_uid(nix::unistd::geteuid())
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten()?;
    let local = docker_socket_path().ok()?;
    let explicit = std::env::var("DOCKER_HOST").ok();
    let mut seen = HashSet::new();
    seen.insert(local.clone());
    if let Ok(path) = tokio::fs::canonicalize(local).await {
        seen.insert(path);
    }
    let normal_marsa = owner.dir.join(".marsa/run/docker.sock");
    seen.insert(normal_marsa.clone());
    if let Ok(path) = tokio::fs::canonicalize(normal_marsa).await {
        seen.insert(path);
    }
    for candidate in candidates(&owner.dir, explicit.as_deref()) {
        let Ok(socket) = tokio::fs::canonicalize(candidate).await else {
            continue;
        };
        if !seen.insert(socket.clone()) {
            continue;
        }
        let Ok(identity) = platform::socket_identity(&socket).await else {
            continue;
        };
        let endpoint = ExternalEndpoint {
            socket: socket.clone(),
            identity,
        };
        let client = endpoint.client(false);
        let (containers, images, volumes, networks) = tokio::join!(
            client.containers(),
            client.images(),
            client.volumes(),
            client.networks()
        );
        if let (Ok(containers), Ok(images)) = (containers, images) {
            let source = source_name(&socket, &owner.dir);
            return Some(ExternalInventory {
                socket,
                endpoint,
                source,
                containers,
                images,
                volumes: volumes.ok(),
                networks: networks.ok(),
            });
        }
    }
    None
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

fn candidates(home: &Path, explicit: Option<&str>) -> impl Iterator<Item = PathBuf> {
    let configured = explicit
        .and_then(|host| host.strip_prefix("unix://"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    configured.into_iter().chain([
        home.join(".docker/run/docker.sock"),
        home.join(".orbstack/run/docker.sock"),
        home.join(".colima/default/docker.sock"),
        home.join(".lima/docker/sock/docker.sock"),
        PathBuf::from("/var/run/docker.sock"),
    ])
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
        let client = ExternalEndpoint { socket, identity }.client(false);
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
            identity: platform::socket_identity(&socket).await.unwrap(),
            socket: socket.clone(),
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

    #[test]
    fn configured_unix_socket_precedes_local_providers() {
        let home = Path::new("/home/test");
        let first = candidates(home, Some("unix:///test/provider.sock")).next();
        assert_eq!(first, Some(PathBuf::from("/test/provider.sock")));
        for host in ["tcp://localhost:2375", "ssh://remote", "unix://relative"] {
            assert_eq!(
                candidates(home, Some(host)).next(),
                Some(home.join(".docker/run/docker.sock"))
            );
        }
    }
}
