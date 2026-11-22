//! Read a bounded snapshot of Docker workloads before allowing the VM to idle.

use std::{io, time::Duration};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Request, StatusCode, body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use marsa_proto::guest::DOCKER_API_PORT;
use marsa_vm::Vm;
use serde::Deserialize;
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_LIST_BYTES: usize = 64 * 1024;
const ACTIVE_CONTAINERS: &str = "/v1.57/containers/json?all=1&filters=%7B%22status%22%3A%5B%22running%22%2C%22paused%22%2C%22restarting%22%5D%7D";

#[derive(Deserialize)]
struct Container {
    #[serde(rename = "State")]
    _state: Active,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Active {
    Running,
    Paused,
    Restarting,
}

pub(super) async fn count(vm: &dyn Vm) -> io::Result<u32> {
    timeout(TIMEOUT, list_active(vm)).await?
}

async fn list_active(vm: &dyn Vm) -> io::Result<u32> {
    let stream = vm
        .connect_vsock(DOCKER_API_PORT)
        .await
        .map_err(io::Error::other)?;
    let (mut sender, connection) = http1::Builder::new()
        .max_buf_size(16 * 1024)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    let request = async move {
        let request = Request::get(ACTIVE_CONTAINERS)
            .header("Host", "docker")
            .header("Connection", "close")
            .body(Empty::<Bytes>::new())
            .map_err(io::Error::other)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        if response.status() != StatusCode::OK {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let body = Limited::new(response.into_body(), MAX_LIST_BYTES)
            .collect()
            .await
            .map_err(io::Error::other)?
            .to_bytes();
        let containers: Vec<Container> = serde_json::from_slice(&body).map_err(io::Error::other)?;
        u32::try_from(containers.len()).map_err(io::Error::other)
    };
    let (response, _) = tokio::join!(request, connection);
    response
}
