//! Accept the upstream Hawser 1.0 WebSocket handshake and pin requests to one authenticated session.

use std::{
    collections::HashMap,
    fmt, io,
    net::SocketAddr,
    path::Path,
    sync::{Arc, LazyLock, Weak},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Limited, StreamBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Frame, Incoming},
    header,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        crypto::ring,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
    },
};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{ErrorResponse, Request as WebRequest, Response as WebResponse},
        protocol::WebSocketConfig,
    },
};

use super::super::{
    ContainerId, DockerError, ExecControl, ExecOutput, MAX_TERMINAL_INPUT, TerminalSize,
};
use super::{Body, DockerTransport, full, proxy, read_pem, valid_token};
use crate::runtime::{Receiver, Sender};

/// The WebSocket path to set in an Edge agent's `DOCKHAND_SERVER_URL`.
pub const HAWSER_EDGE_PATH: &str = "/api/hawser/edge";
const REQUEST_LIMIT: usize = 24 * 1024 * 1024;
const MESSAGE_LIMIT: usize = 36 * 1024 * 1024;
const MAX_REQUESTS: usize = 24;
const MAX_CONNECTIONS: usize = 32;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);
const AGENT_WAIT: Duration = Duration::from_secs(16);
const PEER_TIMEOUT: Duration = Duration::from_secs(90);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(70);
const WRITE_TIMEOUT: Duration = Duration::from_secs(8);

static LISTENERS: LazyLock<Mutex<HashMap<SocketAddr, Weak<Listener>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

type Socket = WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>;

struct Listener {
    certificate: [u8; 32],
    registrations: Arc<Mutex<Vec<Weak<Registration>>>>,
    task: JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Registration {
    agent_id: String,
    token: [u8; 32],
    session: watch::Sender<Option<mpsc::Sender<Command>>>,
    lease: watch::Sender<()>,
}

#[derive(Clone)]
pub(crate) struct EdgeEndpoint {
    address: SocketAddr,
    listener: Arc<Listener>,
    registration: Arc<Registration>,
}

impl fmt::Debug for EdgeEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HawserEdge")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl PartialEq for EdgeEndpoint {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.listener, &other.listener)
            && Arc::ptr_eq(&self.registration, &other.registration)
    }
}

impl Eq for EdgeEndpoint {}

impl EdgeEndpoint {
    pub(in crate::docker) async fn listen(
        address: SocketAddr,
        agent_id: &str,
        cert: &Path,
        key: &Path,
        token: &str,
    ) -> io::Result<Self> {
        if address.port() < 1024 || !valid_token(token) || !valid_agent_id(agent_id) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let (cert, key) = tokio::try_join!(read_pem(cert, false), read_pem(key, true))?;
        let certificate = Sha256::new()
            .chain_update(Sha256::digest(&cert))
            .chain_update(Sha256::digest(&key))
            .finalize()
            .into();
        let listener = {
            let mut listeners = LISTENERS.lock().await;
            listeners.retain(|_, listener| listener.strong_count() > 0);
            if let Some(listener) = listeners.get(&address).and_then(Weak::upgrade) {
                if listener.certificate != certificate {
                    return Err(io::ErrorKind::AddrInUse.into());
                }
                listener
            } else {
                let chain: Vec<_> = CertificateDer::pem_slice_iter(&cert)
                    .collect::<Result<_, _>>()
                    .map_err(io::Error::other)?;
                let key = PrivateKeyDer::from_pem_slice(&key).map_err(io::Error::other)?;
                let config =
                    ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
                        .with_safe_default_protocol_versions()
                        .map_err(io::Error::other)?
                        .with_no_client_auth()
                        .with_single_cert(chain, key)
                        .map_err(io::Error::other)?;
                let socket = std::net::TcpListener::bind(address)?;
                socket.set_nonblocking(true)?;
                let socket = TcpListener::from_std(socket)?;
                let registrations = Arc::new(Mutex::new(Vec::new()));
                let task = tokio::spawn(serve(
                    socket,
                    TlsAcceptor::from(Arc::new(config)),
                    registrations.clone(),
                ));
                let listener = Arc::new(Listener {
                    certificate,
                    registrations,
                    task,
                });
                listeners.insert(address, Arc::downgrade(&listener));
                listener
            }
        };
        let token: [u8; 32] = Sha256::digest(token).into();
        let registration = {
            let mut registrations = listener.registrations.lock().await;
            registrations.retain(|entry| entry.strong_count() > 0);
            if let Some(existing) = registrations
                .iter()
                .filter_map(Weak::upgrade)
                .find(|entry| entry.agent_id == agent_id && bool::from(entry.token.ct_eq(&token)))
            {
                existing
            } else {
                if registrations.len() >= 64 {
                    return Err(io::ErrorKind::ResourceBusy.into());
                }
                let (session, _) = watch::channel(None);
                let registration = Arc::new(Registration {
                    agent_id: agent_id.to_owned(),
                    token,
                    session,
                    lease: watch::channel(()).0,
                });
                registrations.push(Arc::downgrade(&registration));
                registration
            }
        };
        Ok(Self {
            address,
            listener,
            registration,
        })
    }

    pub(in crate::docker) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(in crate::docker) async fn wait(&self) -> io::Result<()> {
        let mut receiver = self.registration.session.subscribe();
        timeout(AGENT_WAIT, receiver.wait_for(Option::is_some))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(|_| io::ErrorKind::NotConnected)?;
        Ok(())
    }

    pub(in crate::docker) fn connect(&self) -> io::Result<DockerTransport> {
        let sender = self
            .registration
            .session
            .borrow()
            .clone()
            .ok_or(io::ErrorKind::NotConnected)?;
        Ok(proxy(move |request| {
            let sender = sender.clone();
            async move { forward(sender, request).await }
        }))
    }

    pub(in crate::docker) async fn open_terminal(
        &self,
        container: &ContainerId,
        size: TerminalSize,
    ) -> Result<EdgeTerminal, DockerError> {
        let sender = self
            .registration
            .session
            .borrow()
            .clone()
            .ok_or_else(|| DockerError::Socket(io::ErrorKind::NotConnected.into()))?;
        let id = crate::request_id().map_err(DockerError::TerminalIo)?;
        let (result, response) = mpsc::channel(12);
        let mut terminal = EdgeTerminal {
            sender,
            id,
            response,
        };
        let [rows, cols] = size.docker();
        // Hawser executes one program name. It cannot pass the argument vector
        // used by the native Docker transport's login-shell bootstrap.
        let request = json!({
            "type":"exec_start", "execId":terminal.id, "containerId":container.as_str(),
            "cmd":"/bin/sh", "cols":cols, "rows":rows,
        });
        terminal
            .sender
            .try_send(Command::ExecStart {
                id: terminal.id.clone(),
                request,
                result,
            })
            .map_err(|_| DockerError::TerminalIo(io::ErrorKind::NotConnected.into()))?;
        if !matches!(
            timeout(HANDSHAKE_TIMEOUT, terminal.response.recv()).await,
            Ok(Some(ExecMessage::Ready))
        ) {
            return Err(DockerError::TerminalUpgrade);
        }
        Ok(terminal)
    }
}

pub(in crate::docker) fn valid_agent_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

async fn serve(
    socket: TcpListener,
    tls: TlsAcceptor,
    registrations: Arc<Mutex<Vec<Weak<Registration>>>>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = connections.join_next(), if !connections.is_empty() => {}
            incoming = socket.accept(), if connections.len() < MAX_CONNECTIONS => {
                let Ok((stream, _)) = incoming else { return };
                let tls = tls.clone();
                let registrations = registrations.clone();
                connections.spawn(async move {
                    if let Ok(Ok((socket, registration))) = timeout(
                        HANDSHAKE_TIMEOUT, authenticate(stream, tls, registrations),
                    ).await {
                        drive(socket, registration).await;
                    }
                });
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    protocol: String,
    agent_id: String,
    #[serde(default)]
    token: String,
    token_hash: Option<String>,
}

impl Hello {
    fn digest(&self) -> Option<[u8; 32]> {
        if let Some(hash) = &self.token_hash {
            let mut digest = [0; 32];
            let bytes = hash.as_bytes();
            if bytes.len() != digest.len() * 2 {
                return None;
            }
            for (pair, byte) in bytes.as_chunks::<2>().0.iter().zip(&mut digest) {
                let hex = std::str::from_utf8(pair).ok()?;
                *byte = u8::from_str_radix(hex, 16).ok()?;
            }
            Some(digest)
        } else {
            valid_token(&self.token).then(|| Sha256::digest(&self.token).into())
        }
    }
}

#[allow(clippy::result_large_err)] // Tungstenite defines the handshake callback signature.
fn verify_path(request: &WebRequest, response: WebResponse) -> Result<WebResponse, ErrorResponse> {
    if request
        .uri()
        .path_and_query()
        .is_some_and(|uri| uri.as_str() == HAWSER_EDGE_PATH)
    {
        Ok(response)
    } else {
        let mut response = ErrorResponse::new(None);
        *response.status_mut() = StatusCode::NOT_FOUND;
        Err(response)
    }
}

async fn authenticate(
    tcp: TcpStream,
    tls: TlsAcceptor,
    registrations: Arc<Mutex<Vec<Weak<Registration>>>>,
) -> io::Result<(Socket, Arc<Registration>)> {
    let tls = tls.accept(tcp).await?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MESSAGE_LIMIT))
        .max_frame_size(Some(MESSAGE_LIMIT));
    let mut socket = accept_hdr_async_with_config(tls, verify_path, Some(config))
        .await
        .map_err(io::Error::other)?;
    let hello = match socket.next().await {
        Some(Ok(Message::Text(message))) if message.len() <= 16 * 1024 => {
            serde_json::from_str::<Hello>(&message).ok()
        }
        _ => None,
    }
    .filter(|hello| {
        hello.kind == "hello" && hello.protocol == "1.0" && valid_agent_id(&hello.agent_id)
    });
    let registration = if let Some((hello, digest)) = hello
        .as_ref()
        .and_then(|hello| hello.digest().map(|digest| (hello, digest)))
    {
        registrations
            .lock()
            .await
            .iter()
            .filter_map(Weak::upgrade)
            .find(|entry| {
                entry.agent_id == hello.agent_id && bool::from(entry.token.ct_eq(&digest))
            })
    } else {
        None
    };
    let Some(registration) = registration else {
        let _ = socket
            .send(Message::Text(
                json!({"type":"error","error":"Hawser authentication failed"})
                    .to_string()
                    .into(),
            ))
            .await;
        return Err(io::ErrorKind::PermissionDenied.into());
    };
    if registration.session.borrow().is_some() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    socket
        .send(Message::Text(
            json!({"type":"welcome","environmentId":1,"message":"Marsa connected"})
                .to_string()
                .into(),
        ))
        .await
        .map_err(io::Error::other)?;
    Ok((socket, registration))
}

enum Command {
    Request {
        id: String,
        request: Value,
        result: mpsc::Sender<Inbound>,
    },
    EndStream(String),
    ExecStart {
        id: String,
        request: Value,
        result: mpsc::Sender<ExecMessage>,
    },
    ExecSend {
        id: String,
        request: Value,
    },
    ExecEnd(String),
}

enum ExecMessage {
    Ready,
    Output(Bytes),
    End,
}

pub(in crate::docker) struct EdgeTerminal {
    sender: mpsc::Sender<Command>,
    id: String,
    response: mpsc::Receiver<ExecMessage>,
}

impl Drop for EdgeTerminal {
    fn drop(&mut self) {
        let _ = self.sender.try_send(Command::ExecEnd(self.id.clone()));
    }
}

impl EdgeTerminal {
    pub(in crate::docker) async fn drive(
        mut self,
        mut input: Receiver<ExecControl>,
        output: &Sender<Result<ExecOutput, DockerError>>,
    ) -> Result<(), DockerError> {
        if !output.send(Ok(ExecOutput::Opened)).await {
            return Ok(());
        }
        loop {
            tokio::select! {
                message = self.response.recv() => match message {
                    Some(ExecMessage::Output(bytes)) => {
                        if !output.send(Ok(ExecOutput::Bytes(bytes.to_vec()))).await {
                            return Ok(());
                        }
                    }
                    _ => return Ok(()),
                },
                action = input.next() => {
                    let request = match action {
                        Some(ExecControl::Input(bytes)) if bytes.len() <= MAX_TERMINAL_INPUT => {
                            json!({"type":"exec_input", "execId":self.id, "data":STANDARD.encode(bytes)})
                        }
                        Some(ExecControl::Input(_)) => return Err(DockerError::RecordLimit),
                        Some(ExecControl::Resize(size)) => {
                            let [rows, cols] = size.docker();
                            json!({"type":"exec_resize", "execId":self.id, "rows":rows, "cols":cols})
                        }
                        None => return Ok(()),
                    };
                    self.sender.send(Command::ExecSend { id: self.id.clone(), request }).await
                        .map_err(|_| DockerError::TerminalIo(io::ErrorKind::NotConnected.into()))?;
                }
            }
        }
    }
}

enum Inbound {
    Complete(Response<Body>),
    Stream(Bytes),
    End,
}

struct SessionGuard {
    registration: Weak<Registration>,
    sender: mpsc::Sender<Command>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let Some(registration) = self.registration.upgrade() else {
            return;
        };
        registration.session.send_if_modified(|active| {
            if active
                .as_ref()
                .is_some_and(|active| active.same_channel(&self.sender))
            {
                *active = None;
                true
            } else {
                false
            }
        });
    }
}

async fn drive(mut socket: Socket, registration: Arc<Registration>) {
    let (sender, mut commands) = mpsc::channel(MAX_REQUESTS);
    if !registration.session.send_if_modified(|active| {
        if active.is_some() {
            return false;
        }
        *active = Some(sender.clone());
        true
    }) {
        return;
    }
    let mut lease = registration.lease.subscribe();
    let _guard = SessionGuard {
        registration: Arc::downgrade(&registration),
        sender,
    };
    drop(registration);
    let mut pending = HashMap::<String, mpsc::Sender<Inbound>>::new();
    let mut terminals = HashMap::<String, mpsc::Sender<ExecMessage>>::new();
    let peer_deadline = tokio::time::sleep(PEER_TIMEOUT);
    tokio::pin!(peer_deadline);
    loop {
        let data = tokio::select! {
            _ = &mut peer_deadline => break,
            _ = lease.changed() => break,
            received = socket.next() => {
                match received {
                    Some(Ok(Message::Text(data))) => {
                        peer_deadline.as_mut().reset(tokio::time::Instant::now() + PEER_TIMEOUT);
                        receive(&mut pending, &mut terminals, &data)
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                        peer_deadline.as_mut().reset(tokio::time::Instant::now() + PEER_TIMEOUT);
                        continue;
                    }
                    _ => break,
                }
            }
            command = commands.recv() => {
                pending.retain(|_, target| !target.is_closed());
                match command {
                    Some(Command::Request { id, request, result }) if pending.len() < MAX_REQUESTS => {
                        pending.insert(id, result);
                        Some(request)
                    }
                    Some(Command::EndStream(id)) => {
                        pending.remove(&id);
                        Some(json!({"type":"stream_end","requestId":id,"reason":"closed"}))
                    }
                    Some(Command::ExecStart { id, request, result }) if terminals.len() < MAX_REQUESTS => {
                        terminals.insert(id, result);
                        Some(request)
                    }
                    Some(Command::ExecSend { id, request }) if terminals.contains_key(&id) => Some(request),
                    Some(Command::ExecEnd(id)) if terminals.remove(&id).is_some() => {
                        Some(json!({"type":"exec_end", "execId":id, "reason":"closed"}))
                    }
                    Some(_) => None,
                    None => break,
                }
            }
        };
        if let Some(data) = data
            && !matches!(
                timeout(
                    WRITE_TIMEOUT,
                    socket.send(Message::Text(data.to_string().into()))
                )
                .await,
                Ok(Ok(()))
            )
        {
            break;
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMessage {
    #[serde(rename = "type")]
    kind: String,
    request_id: Option<String>,
    exec_id: Option<String>,
    status_code: Option<u16>,
    #[serde(default)]
    headers: HashMap<String, String>,
    body: Option<String>,
    data: Option<String>,
    #[serde(default)]
    is_binary: bool,
    timestamp: Option<i64>,
}

fn receive(
    pending: &mut HashMap<String, mpsc::Sender<Inbound>>,
    terminals: &mut HashMap<String, mpsc::Sender<ExecMessage>>,
    data: &str,
) -> Option<Value> {
    let message: AgentMessage = serde_json::from_str(data).ok()?;
    if message.kind == "ping" {
        return Some(json!({"type":"pong","timestamp":message.timestamp.unwrap_or_default()}));
    }
    if message.kind.starts_with("exec_") || message.kind == "error" {
        let id = message
            .exec_id
            .as_deref()
            .or(message.request_id.as_deref())?;
        if let Some(terminal) = terminals.get(id) {
            let output = match message.kind.as_str() {
                "exec_ready" => ExecMessage::Ready,
                "exec_output" => {
                    let bytes = message
                        .data
                        .as_deref()
                        .filter(|data| data.len() <= 128 * 1024)
                        .and_then(|data| STANDARD.decode(data).ok())?;
                    ExecMessage::Output(Bytes::from(bytes))
                }
                "exec_end" | "error" => {
                    if let Some(terminal) = terminals.remove(id) {
                        let _ = terminal.try_send(ExecMessage::End);
                    }
                    return None;
                }
                _ => return None,
            };
            if terminal.try_send(output).is_err() {
                terminals.remove(id);
                return Some(json!({"type":"exec_end", "execId":id, "reason":"backpressure"}));
            }
            return None;
        }
    }
    let id = message.request_id.as_deref().filter(|id| id.len() == 36)?;
    match message.kind.as_str() {
        "response" => {
            if let Some(target) = pending.remove(id) {
                let result = parse_response(message);
                let _ = target.try_send(result);
            }
        }
        "stream" => {
            let result = message
                .data
                .filter(|data| data.len() <= 128 * 1024)
                .and_then(|data| STANDARD.decode(data).ok())
                .map(Bytes::from);
            if let (Some(target), Some(bytes)) = (pending.get(id), result)
                && target.try_send(Inbound::Stream(bytes)).is_err()
            {
                pending.remove(id);
                return Some(json!({"type":"stream_end","requestId":id,"reason":"backpressure"}));
            }
        }
        "stream_end" | "error" => {
            if let Some(target) = pending.remove(id) {
                let _ = target.try_send(Inbound::End);
            }
        }
        _ => {}
    }
    None
}

fn parse_response(message: AgentMessage) -> Inbound {
    let Some(status) = message
        .status_code
        .and_then(|status| StatusCode::from_u16(status).ok())
        .filter(|status| status.as_u16() >= 200)
    else {
        return Inbound::End;
    };
    let contents = message.body.unwrap_or_default();
    let bytes = if message.is_binary {
        let Ok(data) = STANDARD.decode(contents) else {
            return Inbound::End;
        };
        Bytes::from(data)
    } else {
        Bytes::from(contents)
    };
    let mut response = Response::new(full(bytes));
    *response.status_mut() = status;
    if message.headers.len() > 64 {
        return Inbound::End;
    }
    for (name, value) in message.headers {
        if let (Ok(name), Ok(value)) = (
            header::HeaderName::try_from(name),
            header::HeaderValue::try_from(value),
        ) && !hop_header(&name)
        {
            response.headers_mut().insert(name, value);
        }
    }
    Inbound::Complete(response)
}

fn hop_header(name: &header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "trailer"
            | "te"
            | "expect"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "x-hawser-token"
    )
}

async fn forward(
    sender: mpsc::Sender<Command>,
    request: Request<Incoming>,
) -> io::Result<Response<Body>> {
    if request.headers().contains_key(header::UPGRADE) {
        return Err(io::ErrorKind::Unsupported.into());
    }
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .ok_or(io::ErrorKind::InvalidInput)?
        .as_str();
    if !path.starts_with('/')
        || path.len() > 8192
        || path.starts_with("//")
        || path.starts_with("/_hawser/")
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let route = parts.uri.path();
    let query = parts.uri.query().unwrap_or_default();
    let param = |name| query.split('&').any(|value| value == name);
    let streaming = parts.method == Method::GET
        && ((route.ends_with("/logs") && (param("follow=1") || param("follow=true")))
            || (route.ends_with("/stats") && (param("stream=1") || param("stream=true")))
            || (route.ends_with("/events")
                && !query.split('&').any(|value| value.starts_with("until="))));
    let body = timeout(REQUEST_TIMEOUT, Limited::new(body, REQUEST_LIMIT).collect())
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
        .map_err(io::Error::other)?
        .to_bytes();
    let mut headers = HashMap::<&str, &str>::new();
    for (name, value) in &parts.headers {
        if !hop_header(name) {
            headers.insert(name.as_str(), value.to_str().map_err(io::Error::other)?);
        }
    }
    let id = crate::request_id()?;
    let mut request = json!({"type":"request","requestId":id,"method":parts.method.as_str(),"path":path,"headers":headers,"streaming":streaming});
    if !body.is_empty() {
        request["body"] = Value::String(STANDARD.encode(&body));
        request["isBinary"] = Value::Bool(true);
    }
    let (result, mut response) = mpsc::channel(12);
    sender
        .try_send(Command::Request {
            id: id.clone(),
            request,
            result,
        })
        .map_err(|_| io::ErrorKind::NotConnected)?;
    if streaming {
        let (closed, cancel) = oneshot::channel::<()>();
        let stream =
            futures_util::stream::unfold((response, closed), |(mut response, closed)| async move {
                match response.recv().await {
                    Some(Inbound::Stream(bytes)) => {
                        Some((Ok(Frame::data(bytes)), (response, closed)))
                    }
                    _ => None,
                }
            });
        tokio::spawn(async move {
            let _ = cancel.await;
            let _ = sender.try_send(Command::EndStream(id));
        });
        let mut response = Response::new(StreamBody::new(stream).boxed_unsync());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/octet-stream"),
        );
        return Ok(response);
    }
    match timeout(REQUEST_TIMEOUT, response.recv())
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
    {
        Some(Inbound::Complete(response)) => Ok(response),
        _ => Err(io::ErrorKind::ConnectionAborted.into()),
    }
}
