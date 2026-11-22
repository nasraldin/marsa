//! Register the root-only Docker AuthZ callback and the host-only bind policy stream.

use std::{
    convert::Infallible,
    fs, io,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::Path,
    time::Duration,
};

use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Serialize;
use tokio::{
    net::UnixListener,
    task::JoinSet,
    time::{sleep, timeout},
};

use crate::authorization::{self, Decision, PLUGIN, Policy, socket};

use super::vsock;

const DIRECTORY: &str = "/run/docker/plugins";
const CONNECTIONS: usize = 16;
// Moby sends a JSON envelope around up to 4 MiB of request JSON as base64.
// The remaining room is for the URI and headers in the request callback.
const CALLBACK_BYTES: usize = 12 * 1024 * 1024;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn bind_plugin() -> io::Result<UnixListener> {
    protected_directory(Path::new("/run"))?;
    create_directory(Path::new("/run/docker"))?;
    let directory = Path::new(DIRECTORY);
    create_directory(directory)?;
    let endpoint = directory.join(format!("{PLUGIN}.sock"));
    match fs::symlink_metadata(&endpoint) {
        Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == 0 => {
            fs::remove_file(&endpoint)?;
        }
        Ok(_) => return Err(unprotected_endpoint()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(&endpoint)?;
    fs::set_permissions(endpoint, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn create_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => protected_directory(path),
        Err(error) => Err(error),
    }
}

fn protected_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0 {
        Ok(())
    } else {
        Err(unprotected_endpoint())
    }
}

fn unprotected_endpoint() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unprotected Docker AuthZ endpoint",
    )
}

pub(super) async fn serve_host(listener: vsock::Listener, policy: Policy) {
    loop {
        match listener.accept_host().await {
            Ok(stream) => policy.serve(stream).await,
            Err(error) => {
                eprintln!("marsa-agent: accept bind policy connection: {error}");
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

pub(super) async fn serve_plugin(listener: UnixListener, policy: Policy) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            connection = listener.accept(), if connections.len() < CONNECTIONS => {
                match connection {
                    Ok((stream, _)) if stream.peer_cred().is_ok_and(|peer| peer.uid() == 0) => {
                        let policy = policy.clone();
                        connections.spawn(async move {
                            let _ = http1::Builder::new()
                                .timer(TokioTimer::new())
                                .header_read_timeout(CALLBACK_TIMEOUT)
                                .serve_connection(TokioIo::new(stream), service_fn(|request| async {
                                    Ok::<_, Infallible>(callback(request, &policy).await)
                                }))
                                .await;
                        });
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("marsa-agent: accept Docker AuthZ connection: {error}");
                        sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn callback(
    request: Request<Incoming>,
    policy: &Policy,
) -> Response<Full<hyper::body::Bytes>> {
    if request.method() != Method::POST {
        return response(StatusCode::METHOD_NOT_ALLOWED, Vec::new());
    }
    let path = request.uri().path();
    if path == "/Plugin.Activate" {
        return response(StatusCode::OK, br#"{"Implements":["authz"]}"#.to_vec());
    }
    if path != "/AuthZPlugin.AuthZReq" && path != "/AuthZPlugin.AuthZRes" {
        return response(StatusCode::NOT_FOUND, Vec::new());
    }
    let inspect_request = path == "/AuthZPlugin.AuthZReq";
    let mut body = request.into_body();
    let decision = timeout(CALLBACK_TIMEOUT, async {
        if inspect_request {
            let body = Limited::new(body, CALLBACK_BYTES)
                .collect()
                .await
                .ok()?
                .to_bytes();
            Some(authorization::authorize(&body, policy, socket::is_guest_engine_socket()).await)
        } else {
            // Moby may send large JSON responses in a single write. Drain
            // the callback in constant memory without inspecting the data.
            while let Some(frame) = body.frame().await {
                frame.ok()?;
            }
            Some(Decision::Allow)
        }
    })
    .await;
    let Ok(Some(decision)) = decision else {
        return response(StatusCode::BAD_REQUEST, Vec::new());
    };
    let (allowed, message) = match &decision {
        Decision::Allow => (true, ""),
        Decision::Deny(message) => (false, message.as_str()),
    };
    match serde_json::to_vec(&PluginDecision { allowed, message }) {
        Ok(json) => response(StatusCode::OK, json),
        Err(_) => response(StatusCode::INTERNAL_SERVER_ERROR, Vec::new()),
    }
}

#[derive(Serialize)]
struct PluginDecision<'a> {
    #[serde(rename = "Allow")]
    allowed: bool,
    #[serde(rename = "Msg")]
    message: &'a str,
}

fn response(status: StatusCode, json: Vec<u8>) -> Response<Full<hyper::body::Bytes>> {
    let mut response = Response::new(Full::new(json.into()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}
