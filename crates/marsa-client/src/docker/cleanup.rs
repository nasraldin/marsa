//! Project a selected engine's disk usage without granting cleanup authority.

use std::{io, path::Path};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, body::Bytes, client::conn::http1, header};
use hyper_util::rt::TokioIo;
use marsa_proto::host::{
    docker_cleanup::{CleanupSelection, MAX_BODY},
    v1::{PrunePreviewRequest, PrunePreviewResponse},
};
use tokio::time::timeout;

use crate::{ClientError, DaemonEndpoint};

use super::{
    DockerClient, DockerError, ExternalEndpoint, ManagedEndpointError, ManagedEndpointStore,
    ManagedEndpointTransport, VerifiedEngine,
    inventory::ExternalTransport,
    overview::{DISK_PATH, DISK_TIMEOUT},
    stream::ConnectionTask,
};

impl DockerClient {
    /// Read exact review candidates from the already captured endpoint.
    pub async fn cleanup_preview(
        &self,
        request: PrunePreviewRequest,
    ) -> Result<PrunePreviewResponse, DockerError> {
        let selection = CleanupSelection::new(request).map_err(preview_error)?;
        let bytes = timeout(DISK_TIMEOUT, async {
            let (_, response) = self
                .request_body(
                    Method::GET,
                    DISK_PATH,
                    Full::new(Bytes::new()),
                    Default::default(),
                    DISK_TIMEOUT,
                )
                .await?;
            Limited::new(response.body, MAX_BODY)
                .collect()
                .await
                .map(|body| body.to_bytes())
                .map_err(DockerError::Snapshot)
        })
        .await
        .map_err(|_| DockerError::Timeout)??;
        selection.preview(&bytes).map_err(preview_error)
    }
}

/// Result of a scheduled read of a previously verified local Docker engine.
#[derive(Debug)]
pub enum ScheduledCleanup {
    /// The registered Unix engine did not have a running listener.
    Offline,
    /// The registration, socket address or engine identity changed.
    Skipped,
    /// Candidates from an authenticated engine. They grant no cleanup authority.
    Reviewed(PrunePreviewResponse),
}

impl DaemonEndpoint {
    /// Read a scheduled review of an already-running built-in engine.
    pub async fn scheduled_builtin_cleanup(
        &self,
        request: PrunePreviewRequest,
    ) -> Result<ScheduledCleanup, ClientError> {
        let client = match self.connect_running().await {
            Ok(client) => client,
            Err(ClientError::StartupTimeout) => return Ok(ScheduledCleanup::Offline),
            Err(error) => return Err(error),
        };
        match client.disk().prune_preview(request).await {
            Ok(response) => Ok(ScheduledCleanup::Reviewed(response.into_inner())),
            Err(error) if error.code() == tonic::Code::FailedPrecondition => {
                Ok(ScheduledCleanup::Offline)
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl ManagedEndpointStore {
    /// Read a cleanup review from the exact saved local registration and engine.
    pub async fn scheduled_cleanup(
        &self,
        registration: &str,
        expected_socket: &Path,
        expected_engine: &str,
        request: PrunePreviewRequest,
    ) -> Result<ScheduledCleanup, ManagedEndpointError> {
        let Some(entry) = self
            .list()
            .await?
            .into_iter()
            .find(|entry| entry.id == registration)
        else {
            return Ok(ScheduledCleanup::Skipped);
        };
        if !matches!(
            entry.config.transport,
            ManagedEndpointTransport::Unix { .. }
        ) {
            return Ok(ScheduledCleanup::Skipped);
        }
        let transport = entry.config.capture_transport().await?;
        let ExternalTransport::Unix { socket, identity } = &transport else {
            return Ok(ScheduledCleanup::Skipped);
        };
        if identity.is_none() {
            // A missing socket can be a normal stop. A live replacement at a
            // different canonical path still has to be explicitly reselected.
            return match tokio::fs::symlink_metadata(socket).await {
                Err(error) if unavailable(&error) => Ok(ScheduledCleanup::Offline),
                Err(error) => Err(DockerError::Socket(error).into()),
                Ok(_) => Err(DockerError::Socket(io::ErrorKind::PermissionDenied.into()).into()),
            };
        }
        if socket != expected_socket {
            return Ok(ScheduledCleanup::Skipped);
        }
        ExternalEndpoint::managed(entry.id, transport)
            .review_on_verified_socket(expected_engine, request)
            .await
    }
}

impl ExternalEndpoint {
    async fn review_on_verified_socket(
        &self,
        expected_engine: &str,
        request: PrunePreviewRequest,
    ) -> Result<ScheduledCleanup, ManagedEndpointError> {
        let selection = CleanupSelection::new(request).map_err(preview_error)?;
        timeout(DISK_TIMEOUT, async {
            let socket = match self.open_transport().await {
                Ok(socket) => socket,
                Err(error) if unavailable(&error) => return Ok(ScheduledCleanup::Offline),
                Err(error) => return Err(DockerError::Socket(error).into()),
            };
            let (mut sender, connection) = http1::Builder::new()
                .max_buf_size(16 * 1024)
                .handshake(TokioIo::new(socket))
                .await
                .map_err(DockerError::from)?;
            let _connection = ConnectionTask::new(tokio::spawn(connection).abort_handle());
            let identity = scheduled_read(&mut sender, "/info").await?;
            let engine: VerifiedEngine = serde_json::from_slice(&identity)
                .map_err(|_| ManagedEndpointError::InvalidEngineIdentity)?;
            if !engine.valid() {
                return Err(ManagedEndpointError::InvalidEngineIdentity);
            }
            if engine.id != expected_engine {
                return Ok(ScheduledCleanup::Skipped);
            }

            // Reusing the authenticated connection keeps a reconnect from
            // moving the inventory request to a replacement Docker engine.
            let bytes = scheduled_read(&mut sender, DISK_PATH).await?;
            let preview = selection.preview(&bytes).map_err(preview_error)?;
            Ok(ScheduledCleanup::Reviewed(preview))
        })
        .await
        .map_err(|_| DockerError::Timeout)?
    }
}

async fn scheduled_read(
    sender: &mut http1::SendRequest<Full<Bytes>>,
    path: &'static str,
) -> Result<Bytes, DockerError> {
    let request = Request::get(path)
        .header(header::HOST, "docker")
        .body(Full::new(Bytes::new()))?;
    let response = sender.send_request(request).await?;
    if !response.status().is_success() {
        return Err(DockerError::Status(response.status()));
    }
    Limited::new(response.into_body(), MAX_BODY)
        .collect()
        .await
        .map(|body| body.to_bytes())
        .map_err(DockerError::Snapshot)
}

fn unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::NotConnected
    )
}

fn preview_error(status: tonic::Status) -> DockerError {
    DockerError::CleanupPreview(Box::new(status))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use marsa_proto::host::v1::{DiskCategory, PruneApplyCapability};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use crate::{
        docker::{ExternalEndpoint, inventory::ExternalTransport},
        platform,
    };

    use super::*;

    #[tokio::test]
    async fn cleanup_reads_only_the_pinned_engine_and_cannot_apply() {
        let engine = |name: &str| {
            let directory = tempfile::Builder::new()
                .prefix(name)
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in("/tmp")
                .unwrap();
            let path = directory.path().join("docker.sock");
            let listener = UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            (directory, path, listener)
        };
        let (_selected, path, listener) = engine("selected");
        let (_other, _, other) = engine("other");
        let endpoint = ExternalEndpoint::managed(
            "managed:cleanup-fixture".into(),
            ExternalTransport::Unix {
                identity: Some(platform::socket_identity(&path).await.unwrap()),
                socket: path,
            },
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            assert!(request.starts_with(b"GET /system/df?verbose=true HTTP/1.1\r\n"));
            let body = r#"{"VolumeUsage":{"TotalCount":1,"TotalSize":4096,"Reclaimable":4096,"Items":[{"Name":"fixture","Driver":"local","Scope":"local","UsageData":{"RefCount":0,"Size":4096}}]}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let preview = endpoint
            .management_client()
            .cleanup_preview(PrunePreviewRequest {
                categories: vec![DiskCategory::Volumes.into(), DiskCategory::Images.into()],
                volume_names: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            preview.apply_capability(),
            PruneApplyCapability::ExactDeletionUnsupported
        );
        assert_eq!(preview.candidates.len(), 1);
        assert_eq!(preview.candidates[0].name, "fixture");
        assert_eq!(preview.candidates[0].size_bytes, Some(4096));
        assert_eq!(
            preview.unavailable_categories,
            [DiskCategory::Images as i32]
        );
        assert_eq!(preview.estimated_reclaimable_bytes, None);
        server.await.unwrap();
        assert!(
            timeout(std::time::Duration::from_millis(50), other.accept())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn scheduled_cleanup_detects_replacement_before_reading_candidates() {
        check_scheduled_cleanup("replacement", false).await;
    }

    #[tokio::test]
    async fn scheduled_cleanup_reads_candidates_on_the_verified_connection() {
        check_scheduled_cleanup("original", true).await;
    }

    async fn check_scheduled_cleanup(engine_id: &str, reviews: bool) {
        let directory = tempfile::Builder::new()
            .prefix("scheduled-review")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let path = directory.path().join("docker.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let endpoint = ExternalEndpoint::test_unix_socket(&path).await.unwrap();
        let engine_id = engine_id.to_owned();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert!(
                read_request(&mut socket)
                    .await
                    .starts_with(b"GET /info HTTP/1.1\r\n")
            );
            let info =
                format!(r#"{{"ID":"{engine_id}","Name":"fixture","ServerVersion":"29.10.0"}}"#);
            respond(&mut socket, &info).await;
            if reviews {
                assert!(
                    read_request(&mut socket)
                        .await
                        .starts_with(b"GET /system/df?verbose=true HTTP/1.1\r\n")
                );
                respond(&mut socket, r#"{"VolumeUsage":{"TotalCount":1,"TotalSize":12,"Reclaimable":12,"Items":[{"Name":"fixture","Driver":"local","Scope":"local","UsageData":{"RefCount":0,"Size":12}}]}}"#).await;
            }
            assert_eq!(socket.read(&mut [0]).await.unwrap(), 0);
        });
        let result = endpoint
            .review_on_verified_socket(
                "original",
                PrunePreviewRequest {
                    categories: vec![DiskCategory::Volumes.into()],
                    volume_names: Vec::new(),
                },
            )
            .await
            .unwrap();
        match (reviews, result) {
            (true, ScheduledCleanup::Reviewed(review)) => {
                assert_eq!(review.candidates[0].name, "fixture");
                assert_eq!(
                    review.apply_capability(),
                    PruneApplyCapability::ExactDeletionUnsupported
                );
            }
            (false, ScheduledCleanup::Skipped) => {}
            (_, result) => panic!("unexpected scheduled result: {result:?}"),
        }
        server.await.unwrap();
    }

    async fn read_request(socket: &mut tokio::net::UnixStream) -> Vec<u8> {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        request
    }

    async fn respond(socket: &mut tokio::net::UnixStream, body: &str) {
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }
}
