//! Give the bundled Compose process a temporary socket backed by the pinned Docker transport.

use std::{io, path::PathBuf, time::Duration};

use tokio::{
    io::copy_bidirectional,
    net::UnixListener,
    task::{JoinHandle, JoinSet},
    time::timeout,
};

use crate::docker::ExternalEndpoint;

const MAX_CONNECTIONS: usize = 32;

pub(super) struct Relay {
    _directory: tempfile::TempDir,
    socket: PathBuf,
    worker: JoinHandle<()>,
}

impl Relay {
    pub(super) fn start(endpoint: ExternalEndpoint) -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::Builder::new()
            .prefix("marsa-compose.")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?;
        let socket = directory.path().join("docker.sock");
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let worker = tokio::spawn(serve(listener, endpoint));
        Ok(Self {
            _directory: directory,
            socket,
            worker,
        })
    }

    pub(super) fn socket(&self) -> &std::path::Path {
        &self.socket
    }
}

async fn serve(listener: UnixListener, endpoint: ExternalEndpoint) {
    let mut connections = JoinSet::new();
    let owner = nix::unistd::geteuid().as_raw();
    loop {
        tokio::select! {
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            result = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let Ok((mut local, _)) = result else { return };
                if local.peer_cred().is_ok_and(|peer| peer.uid() == owner) {
                    let endpoint = endpoint.clone();
                    connections.spawn(async move {
                        if let Ok(Ok(mut docker)) = timeout(Duration::from_secs(10), endpoint.open_transport()).await {
                            let _ = copy_bidirectional(&mut local, &mut docker).await;
                        }
                    });
                }
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.worker.abort();
    }
}
