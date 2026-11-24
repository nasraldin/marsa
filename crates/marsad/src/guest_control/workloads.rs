//! Read a bounded snapshot of Docker workloads before allowing the VM to idle.

use std::{io, time::Duration};

use marsa_proto::guest::DOCKER_API_PORT;
use marsa_vm::Vm;
use serde::Deserialize;
use tokio::time::timeout;

use super::docker_http;

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
    let body = docker_http::get(stream, ACTIVE_CONTAINERS, MAX_LIST_BYTES).await?;
    let containers: Vec<Container> = serde_json::from_slice(&body).map_err(io::Error::other)?;
    u32::try_from(containers.len()).map_err(io::Error::other)
}
