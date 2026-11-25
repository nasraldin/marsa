//! Report an owner-requested snapshot of unused guest storage without editing Docker state.

use std::{io, time::Duration};

use marsa_proto::host::{
    docker_cleanup::{CleanupSelection, MAX_BODY},
    v1::{PrunePreviewRequest, PrunePreviewResponse},
};
use tokio::time::timeout;
use tonic::Status;

use crate::guest_control::docker_http;

use super::TypedApi;

const TIMEOUT: Duration = Duration::from_secs(10);
impl TypedApi {
    pub(super) async fn preview_disk_cleanup(
        &self,
        request: PrunePreviewRequest,
    ) -> Result<PrunePreviewResponse, Status> {
        let selection = CleanupSelection::new(request)?;
        let supervisor = self.supervisor.as_ref().ok_or_else(not_running)?;
        let uri = selection.uri();
        let bytes = timeout(TIMEOUT, async {
            let stream = supervisor.connect_running_docker().await?;
            docker_http::get(stream, &uri, MAX_BODY).await
        })
        .await
        .map_err(|_| Status::deadline_exceeded("Docker disk inventory timed out"))?
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotConnected => not_running(),
            _ => Status::unavailable("could not inspect Docker disk inventory"),
        })?;
        selection.preview(&bytes)
    }
}

fn not_running() -> Status {
    Status::failed_precondition("disk cleanup preview requires a running VM")
}
