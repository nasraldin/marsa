//! Hawser Standard's authenticated HTTP proxy and the Edge reverse tunnel.

mod edge;

use std::{fmt, io, path::Path, sync::Arc, time::Duration};

use http_body_util::{BodyExt as _, Full};
use hyper::{
    Request, Response, StatusCode,
    body::{Bytes, Incoming},
    client::conn::http1,
    header,
    server::conn::http1 as server,
    service::service_fn,
    upgrade,
};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{copy_bidirectional, duplex},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{
    TlsConnector,
    rustls::{
        ClientConfig, RootCertStore,
        crypto::ring,
        pki_types::{CertificateDer, ServerName, pem::PemObject as _},
    },
};

use super::{
    DockerTransport,
    contexts::{TlsEndpoint, read_pem},
    registry::helper,
};
use crate::platform;

pub use edge::HAWSER_EDGE_PATH;
pub(super) use edge::{EdgeEndpoint, EdgeTerminal, valid_agent_id};
pub(super) const TEST_TIMEOUT: Duration = Duration::from_secs(24);
type Body = http_body_util::combinators::UnsyncBoxBody<Bytes, io::Error>;

const TOKEN_HEADER: &str = "x-hawser-token";
const INFO: &str = "/_hawser/info";

pub(super) fn valid_token(value: &str) -> bool {
    (16..=512).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn credential_key(reference: &str, target: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(target);
    let mut key = format!("https://hawser.credentials.marsa.invalid/{reference}/");
    for byte in digest {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

pub(super) async fn load_secret(reference: &str, target: &str) -> io::Result<String> {
    let helper_path = platform::bundled_registry_helper().await?;
    helper::get(&helper_path, &credential_key(reference, target))
        .await?
        .filter(|credential| credential.username == "hawser" && valid_token(&credential.secret))
        .map(|credential| credential.secret)
        .ok_or_else(|| io::ErrorKind::NotFound.into())
}

pub(super) async fn save_secret(reference: &str, target: &str, secret: String) -> io::Result<()> {
    let helper_path = platform::bundled_registry_helper().await?;
    helper::store(
        &helper_path,
        &helper::Credential {
            server_url: credential_key(reference, target),
            username: "hawser".into(),
            secret,
        },
    )
    .await
}

pub(super) async fn remove_secret(reference: &str, target: &str) -> io::Result<()> {
    let helper_path = platform::bundled_registry_helper().await?;
    helper::erase(&helper_path, &credential_key(reference, target)).await
}

pub(super) fn standard_address(
    address: &str,
) -> io::Result<(hyper::http::uri::Authority, ServerName<'static>)> {
    let address = address
        .strip_prefix("https://")
        .ok_or(io::ErrorKind::InvalidInput)?;
    TlsEndpoint::parse_address(address)
}

#[derive(Clone)]
pub(super) struct StandardEndpoint {
    authority: hyper::http::uri::Authority,
    server_name: ServerName<'static>,
    tls: Arc<ClientConfig>,
    token: header::HeaderValue,
    identity: [u8; 32],
    verified: Arc<tokio::sync::OnceCell<()>>,
}

impl fmt::Debug for StandardEndpoint {
    fn fmt(&self, format: &mut fmt::Formatter<'_>) -> fmt::Result {
        format
            .debug_struct("HawserStandard")
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl PartialEq for StandardEndpoint {
    fn eq(&self, other: &Self) -> bool {
        self.authority == other.authority && self.identity == other.identity
    }
}

impl Eq for StandardEndpoint {}

impl StandardEndpoint {
    pub(super) async fn load(address: &str, ca: &Path, token: String) -> io::Result<Self> {
        let (authority, server_name) = standard_address(address)?;
        let pem = read_pem(ca, false).await?;
        let mut roots = RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(&pem) {
            roots
                .add(certificate.map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        }
        if roots.is_empty() || !valid_token(&token) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let tls = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let identity = Sha256::new()
            .chain_update(Sha256::digest(&pem))
            .chain_update(Sha256::digest(&token))
            .finalize()
            .into();
        let mut token = header::HeaderValue::from_str(&token).map_err(io::Error::other)?;
        token.set_sensitive(true);
        Ok(Self {
            authority,
            server_name,
            tls: Arc::new(tls),
            token,
            identity,
            verified: Arc::new(tokio::sync::OnceCell::new()),
        })
    }

    pub(super) fn authority(&self) -> &str {
        self.authority.as_str()
    }

    async fn socket(&self) -> io::Result<DockerTransport> {
        let tcp = TcpStream::connect(self.authority()).await?;
        let tls = TlsConnector::from(self.tls.clone())
            .connect(self.server_name.clone(), tcp)
            .await?;
        Ok(Box::new(tls))
    }

    pub(super) async fn verify_agent(&self) -> io::Result<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Info {
            agent_id: String,
            hawser_version: String,
            mode: String,
        }
        let response = self
            .forward(Request::get(INFO).body(empty()).map_err(io::Error::other)?)
            .await?;
        if !response.status().is_success() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let body = http_body_util::Limited::new(response.into_body(), 16 * 1024)
            .collect()
            .await
            .map_err(io::Error::other)?
            .to_bytes();
        let info: Info = serde_json::from_slice(&body).map_err(|_| io::ErrorKind::InvalidData)?;
        if info.mode != "standard"
            || !super::managed::valid_text(&info.agent_id, 256)
            || !super::managed::valid_text(&info.hawser_version, 256)
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }

    pub(super) async fn connect(&self) -> io::Result<DockerTransport> {
        self.verified
            .get_or_try_init(|| self.verify_agent())
            .await?;
        let endpoint = self.clone();
        Ok(proxy(move |request| {
            let endpoint = endpoint.clone();
            async move {
                let (parts, body) = request.into_parts();
                endpoint
                    .forward(Request::from_parts(
                        parts,
                        body.map_err(io::Error::other).boxed_unsync(),
                    ))
                    .await
            }
        }))
    }

    async fn forward(&self, mut request: Request<Body>) -> io::Result<Response<Body>> {
        let socket = timeout(Duration::from_secs(8), self.socket())
            .await
            .map_err(|_| io::ErrorKind::TimedOut)??;
        let (mut sender, connection) = http1::Builder::new()
            .max_buf_size(32 * 1024)
            .handshake(TokioIo::new(socket))
            .await
            .map_err(io::Error::other)?;
        let connection = tokio::spawn(connection.with_upgrades());
        let local_upgrade = request
            .headers()
            .contains_key(header::UPGRADE)
            .then(|| upgrade::on(&mut request));
        request
            .headers_mut()
            .insert(TOKEN_HEADER, self.token.clone());
        if local_upgrade.is_none() {
            request.headers_mut().insert(
                header::CONNECTION,
                header::HeaderValue::from_static("close"),
            );
        }
        request.headers_mut().insert(
            header::HOST,
            header::HeaderValue::from_str(self.authority()).map_err(io::Error::other)?,
        );
        let mut response = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        if response.status() == StatusCode::SWITCHING_PROTOCOLS
            && let Some(local) = local_upgrade
        {
            let remote = upgrade::on(&mut response);
            tokio::spawn(async move {
                if let Ok((local, remote)) = tokio::try_join!(local, remote) {
                    let _ = copy_bidirectional(&mut TokioIo::new(local), &mut TokioIo::new(remote))
                        .await;
                }
            });
        }
        drop(connection);
        let (parts, body) = response.into_parts();
        Ok(Response::from_parts(
            parts,
            body.map_err(io::Error::other).boxed_unsync(),
        ))
    }
}

fn proxy<F, Fut>(handler: F) -> DockerTransport
where
    F: Fn(Request<Incoming>) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = io::Result<Response<Body>>> + Send + 'static,
{
    let (docker, bridge) = duplex(64 * 1024);
    tokio::spawn(async move {
        let service = service_fn(move |request| {
            let handler = handler.clone();
            async move {
                Ok::<_, std::convert::Infallible>(
                    handler(request).await.unwrap_or_else(|_| unavailable()),
                )
            }
        });
        let _ = server::Builder::new()
            .max_buf_size(32 * 1024)
            .serve_connection(TokioIo::new(bridge), service)
            .with_upgrades()
            .await;
    });
    Box::new(docker)
}

fn empty() -> Body {
    full(Bytes::new())
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn unavailable() -> Response<Body> {
    let mut response = Response::new(empty());
    *response.status_mut() = StatusCode::BAD_GATEWAY;
    response
}

#[cfg(test)]
mod tests;
