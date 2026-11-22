//! Inspect a separate user-owned Docker engine without changing Marsa's endpoint.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{
    ContainerSummary, DockerClient, ImageSummary, NetworkSummary, VolumeSummary, docker_socket_path,
};

/// Containers and images reported by a detected local Docker engine.
pub struct ExternalInventory {
    /// Authenticated local socket queried for this inventory.
    pub socket: PathBuf,
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
        let client = DockerClient {
            socket: socket.clone(),
            inventory_only: true,
        };
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

        let client = DockerClient {
            socket,
            inventory_only: true,
        };
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
