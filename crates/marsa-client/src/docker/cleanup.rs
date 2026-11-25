//! Project a selected engine's disk usage without granting cleanup authority.

use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, body::Bytes};
use marsa_proto::host::{
    docker_cleanup::{CleanupSelection, MAX_BODY},
    v1::{PrunePreviewRequest, PrunePreviewResponse},
};
use tokio::time::timeout;

use super::{
    DockerClient, DockerError,
    overview::{DISK_PATH, DISK_TIMEOUT},
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
}
