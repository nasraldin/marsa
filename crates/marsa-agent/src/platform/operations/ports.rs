//! Derive guest port transitions from the pinned Docker API's event and inspect responses.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::IpAddr,
};

use http_body_util::{BodyExt, Empty};
use hyper::{
    Request, StatusCode,
    body::{Bytes, Incoming},
    client::conn::http1,
};
use hyper_util::rt::TokioIo;
use marsa_proto::guest::v1::{PortAction, PortEvent, Transport};
use serde::Deserialize;
use tokio::{
    net::UnixStream,
    sync::mpsc::Sender,
    time::{Duration, sleep, timeout},
};

const EVENT_PATH: &str = "/events?filters=%7B%22type%22%3A%5B%22container%22%5D%2C%22event%22%3A%5B%22start%22%2C%22die%22%2C%22destroy%22%5D%7D";
const MAX_EVENT_BYTES: usize = 128 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONTAINERS: usize = 1024;
const MAX_BINDINGS: usize = 65_536;
const ENGINE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
struct DockerEvent {
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Actor")]
    actor: Container,
}

#[derive(Deserialize)]
struct Container {
    #[serde(rename = "Id", alias = "ID")]
    id: String,
}

#[derive(Deserialize)]
struct Inspection {
    #[serde(rename = "NetworkSettings")]
    network: Option<Network>,
}

#[derive(Deserialize)]
struct Network {
    #[serde(rename = "Ports")]
    ports: Option<BTreeMap<String, Option<Vec<HostBinding>>>>,
}

#[derive(Deserialize)]
struct HostBinding {
    #[serde(rename = "HostIp")]
    address: String,
    #[serde(rename = "HostPort")]
    port: String,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Binding {
    container: String,
    port: u16,
    address: IpAddr,
    published: u16,
    transport: Transport,
}

type Bindings = BTreeMap<String, BTreeSet<Binding>>;

pub(in crate::platform) async fn watch(engine_socket: &str, output: Sender<PortEvent>) {
    let mut bindings = Bindings::new();
    while !output.is_closed() {
        let _ = watch_once(engine_socket, &mut bindings, &output).await;
        // A dead engine has no usable bindings. A brief HTTP failure while
        // it is alive should not produce spurious teardown and reopen events.
        if matches!(UnixStream::connect(engine_socket).await, Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound))
            && reconcile(&mut bindings, Bindings::new(), &output)
                .await
                .is_err()
        {
            return;
        }
        sleep(Duration::from_millis(250)).await;
    }
}

async fn watch_once(
    socket: &str,
    bindings: &mut Bindings,
    output: &Sender<PortEvent>,
) -> io::Result<()> {
    // Subscribe before scanning current containers so startup cannot miss a transition.
    let (status, mut events) = request(socket, EVENT_PATH).await?;
    if status != StatusCode::OK {
        return Err(invalid());
    }
    let (status, containers) = request(socket, "/containers/json").await?;
    if status != StatusCode::OK {
        return Err(invalid());
    }
    let containers: Vec<Container> = body_json(containers).await?;
    if containers.len() > MAX_CONTAINERS {
        return Err(invalid());
    }
    let mut running = BTreeMap::new();
    let mut bindings_seen = 0;
    for container in containers {
        let state = inspect(socket, &container.id).await?;
        bindings_seen += state.len();
        if bindings_seen > MAX_BINDINGS {
            return Err(invalid());
        }
        if !state.is_empty() {
            running.insert(container.id, state);
        }
    }
    reconcile(bindings, running, output).await?;

    let mut line = Vec::new();
    while let Some(frame) = events.frame().await {
        let Ok(data) = frame.map_err(io::Error::other)?.into_data() else {
            continue;
        };
        for fragment in data.split_inclusive(|byte| *byte == b'\n') {
            if line.len() + fragment.len() > MAX_EVENT_BYTES {
                return Err(invalid());
            }
            line.extend_from_slice(fragment);
            if fragment.ends_with(b"\n") {
                let event: DockerEvent = serde_json::from_slice(&line).map_err(io::Error::other)?;
                line.clear();
                if !valid_id(&event.actor.id) {
                    return Err(invalid());
                }
                let previous = bindings.remove(&event.actor.id).unwrap_or_default();
                let next = match event.action.as_str() {
                    "start" => match inspect(socket, &event.actor.id).await {
                        Ok(next) => next,
                        Err(error) => {
                            if !previous.is_empty() {
                                bindings.insert(event.actor.id, previous);
                            }
                            return Err(error);
                        }
                    },
                    "die" | "destroy" => BTreeSet::new(),
                    _ => previous.clone(),
                };
                if !next.is_empty() {
                    if bindings.len() == MAX_CONTAINERS
                        || bindings.values().map(BTreeSet::len).sum::<usize>() + next.len()
                            > MAX_BINDINGS
                    {
                        if !previous.is_empty() {
                            bindings.insert(event.actor.id, previous);
                        }
                        return Err(invalid());
                    }
                    bindings.insert(event.actor.id, next.clone());
                }
                emit(&previous, &next, output).await?;
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "Docker events disconnected",
    ))
}

async fn reconcile(
    previous: &mut Bindings,
    next: Bindings,
    output: &Sender<PortEvent>,
) -> io::Result<()> {
    let empty = BTreeSet::new();
    for (id, bindings) in previous.iter() {
        emit(bindings, next.get(id).unwrap_or(&empty), output).await?;
    }
    for (id, bindings) in &next {
        if !previous.contains_key(id) {
            emit(&empty, bindings, output).await?;
        }
    }
    *previous = next;
    Ok(())
}

async fn emit(
    previous: &BTreeSet<Binding>,
    next: &BTreeSet<Binding>,
    output: &Sender<PortEvent>,
) -> io::Result<()> {
    for (set, except, action) in [
        (previous, next, PortAction::Closed),
        (next, previous, PortAction::Opened),
    ] {
        for binding in set.difference(except) {
            output
                .send(PortEvent {
                    action: action as i32,
                    transport: binding.transport as i32,
                    container_id: binding.container.clone(),
                    bind_address: binding.address.to_string(),
                    published_port: u32::from(binding.published),
                    container_port: u32::from(binding.port),
                })
                .await
                .map_err(io::Error::other)?;
        }
    }
    Ok(())
}

async fn inspect(socket: &str, id: &str) -> io::Result<BTreeSet<Binding>> {
    if !valid_id(id) {
        return Err(invalid());
    }
    let (status, body) = request(socket, &format!("/containers/{id}/json")).await?;
    if status == StatusCode::NOT_FOUND {
        return Ok(BTreeSet::new());
    }
    if status != StatusCode::OK {
        return Err(invalid());
    }
    inspect_bindings(id, body_json(body).await?)
}

fn inspect_bindings(id: &str, inspection: Inspection) -> io::Result<BTreeSet<Binding>> {
    let mut bindings = BTreeSet::new();
    if let Some(ports) = inspection.network.and_then(|network| network.ports) {
        for (container, published) in ports {
            let (port, transport) = container.split_once('/').ok_or_else(invalid)?;
            let transport = match transport {
                "tcp" => Transport::Tcp,
                "udp" => Transport::Udp,
                _ => continue,
            };
            let port: u16 = port
                .parse()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(invalid)?;
            for host in published.into_iter().flatten() {
                // Routed-only IPv6 bindings have neither a published host port nor a proxy.
                if host.port.is_empty() {
                    continue;
                }
                let published = host
                    .port
                    .parse()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(invalid)?;
                let address = host.address.parse().map_err(|_| invalid())?;
                bindings.insert(Binding {
                    container: id.to_owned(),
                    port,
                    address,
                    published,
                    transport,
                });
                if bindings.len() > MAX_BINDINGS {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(bindings)
}

async fn request(socket: &str, path: &str) -> io::Result<(StatusCode, Incoming)> {
    timeout(ENGINE_TIMEOUT, async {
        let socket = UnixStream::connect(socket).await?;
        let (mut client, driver) = http1::Builder::new()
            .max_buf_size(32 * 1024)
            .handshake(TokioIo::new(socket))
            .await
            .map_err(io::Error::other)?;
        tokio::spawn(driver);
        let request = Request::get(path)
            .header("Host", "localhost")
            .header("Connection", "close")
            .body(Empty::<Bytes>::new())
            .map_err(io::Error::other)?;
        let response = client
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        Ok((response.status(), response.into_body()))
    })
    .await?
}

async fn body_json<T: serde::de::DeserializeOwned>(mut body: Incoming) -> io::Result<T> {
    timeout(ENGINE_TIMEOUT, async {
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.map_err(io::Error::other)?.into_data() {
                if bytes.len() + data.len() > MAX_RESPONSE_BYTES {
                    return Err(invalid());
                }
                bytes.extend_from_slice(&data);
            }
        }
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    })
    .await?
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or oversized Docker metadata",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bindings_open_and_close_once_for_duplicate_events_and_restarts() {
        let id = "a".repeat(64);
        let live = || {
            inspect_bindings(&id, serde_json::from_str(r#"{"NetworkSettings":{"Ports":{"80/tcp":[{"HostIp":"0.0.0.0","HostPort":"8080"}],"90/udp":[{"HostIp":"::","HostPort":""}]}}}"#).unwrap()).unwrap()
        };
        let (send, mut read) = tokio::sync::mpsc::channel(8);
        let empty = BTreeSet::new();
        let live = live();
        for (from, to) in [
            (&empty, &live),
            (&live, &live),
            (&live, &empty),
            (&empty, &empty),
            (&empty, &live),
        ] {
            emit(from, to, &send).await.unwrap();
        }
        drop(send);
        for expected in [PortAction::Opened, PortAction::Closed, PortAction::Opened] {
            let event = read.recv().await.unwrap();
            assert_eq!(event.action, expected as i32);
            assert_eq!(event.published_port, 8080);
        }
        assert!(read.recv().await.is_none());
    }
}
