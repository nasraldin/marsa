//! Inspect a separate user-owned Docker engine without changing Marsa's endpoint.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use hyper::Method;

use super::{
    ContainerAction, ContainerDetails, ContainerId, ContainerSummary, DockerClient, DockerError,
    DockerLogs, DockerScope, ImageSummary, NetworkSummary, VolumeSummary, docker_socket_path,
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

    /// Start or stop the selected container on this engine.
    pub async fn act(&self, id: &ContainerId, action: ContainerAction) -> Result<(), DockerError> {
        self.client(true).act(id, action).await
    }

    /// Refresh this engine's container snapshot without searching for another endpoint.
    pub async fn containers(&self) -> Result<Vec<ContainerSummary>, DockerError> {
        self.client(false).containers().await
    }
}

pub(super) fn external_route(method: &Method, path: &str, containers: bool) -> bool {
    if method == Method::GET
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
    let Some((id, action)) = path
        .strip_prefix("/containers/")
        .and_then(|tail| tail.split_once('/'))
    else {
        return false;
    };
    if id.len() != 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return false;
    }
    match *method {
        Method::GET => matches!(
            action,
            "json" | "logs?stdout=true&stderr=true&follow=true&tail=200"
        ),
        Method::POST => matches!(action, "start" | "stop"),
        _ => false,
    }
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
        assert!(endpoint.act(&id, ContainerAction::Restart).await.is_err());
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
