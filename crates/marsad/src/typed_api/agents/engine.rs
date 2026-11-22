//! Forward a single reviewed container lifecycle command to the local guest engine.

use std::{future::Future, io, time::Duration};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Request, StatusCode, body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::time::timeout;

use super::super::TypedApi;

const STARTUP_LIMIT: Duration = Duration::from_secs(180);
const COMMAND_LIMIT: Duration = Duration::from_secs(30);
const MAX_INSPECT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) enum StopResult {
    Stopped,
    AlreadyStopped,
    NotFound,
    Refused,
}

#[derive(Deserialize)]
struct Identity {
    #[serde(rename = "Id")]
    id: String,
}

pub(super) async fn stop_container(
    api: &TypedApi,
    id: &str,
    still_authorized: impl Future<Output = bool>,
) -> io::Result<StopResult> {
    let stream = timeout(STARTUP_LIMIT, api.connect_docker()).await??;
    timeout(COMMAND_LIMIT, async {
        let (mut sender, connection) = http1::Builder::new()
            .max_buf_size(16 * 1024)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(io::Error::other)?;
        let operation = async move {
            // Moby can interpret a missing full ID as a name. Confirm identity
            // immediately before acting on it. Only the owner can mutate the
            // Docker name/ID namespace concurrently with this foundation.
            let inspect = Request::get(format!("/v1.57/containers/{id}/json"))
                .header("Host", "docker")
                .body(Empty::<Bytes>::new())
                .map_err(io::Error::other)?;
            let response = sender
                .send_request(inspect)
                .await
                .map_err(io::Error::other)?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(StopResult::NotFound);
            }
            if response.status() != StatusCode::OK {
                return Err(io::Error::other("container identity could not be checked"));
            }
            let body = Limited::new(response.into_body(), MAX_INSPECT_BYTES)
                .collect()
                .await
                .map_err(io::Error::other)?
                .to_bytes();
            let identity: Identity = serde_json::from_slice(&body).map_err(io::Error::other)?;
            if identity.id != id {
                return Ok(StopResult::NotFound);
            }
            if !still_authorized.await {
                return Ok(StopResult::Refused);
            }
            let stop = Request::post(format!("/v1.57/containers/{id}/stop"))
                .header("Host", "docker")
                .header("Connection", "close")
                .body(Empty::<Bytes>::new())
                .map_err(io::Error::other)?;
            let response = sender.send_request(stop).await.map_err(io::Error::other)?;
            match response.status() {
                StatusCode::NO_CONTENT => Ok(StopResult::Stopped),
                StatusCode::NOT_MODIFIED => Ok(StopResult::AlreadyStopped),
                StatusCode::NOT_FOUND => Ok(StopResult::NotFound),
                _ => Err(io::Error::other("Docker rejected the stop request")),
            }
        };
        let (result, _) = tokio::join!(operation, connection);
        result
    })
    .await?
}
