//! Measure direct IPv4 registry connectivity before the guest starts Docker.

mod platform;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};

use http::{
    Request, Response, StatusCode,
    header::{CONNECTION, HOST, WWW_AUTHENTICATE},
};
use http_body_util::Empty;
use hyper::{body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use marsa_proto::guest::v1::{EgressResult, EgressStage, ProbeEgress};
use rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, ServerName, pem::PemObject},
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpStream, UdpSocket},
    sync::Semaphore,
    time::timeout,
};
use tokio_rustls::TlsConnector;

const REGISTRY: &str = "registry-1.docker.io";
const DNS_NAME: &str = "registry-1.docker.io.";
const PORT: u16 = 443;
const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

const ROUTE_LIMIT: Duration = Duration::from_millis(250);
const LOOKUP_LIMIT: Duration = Duration::from_secs(2);
const CONNECT_LIMIT: Duration = Duration::from_secs(2);
const TLS_LIMIT: Duration = Duration::from_secs(2);
const HTTP_LIMIT: Duration = Duration::from_secs(2);
const CA_LOAD_LIMIT: Duration = Duration::from_secs(1);
const CA_BYTES: u64 = 2 * 1024 * 1024;
static RESOLVER: Semaphore = Semaphore::const_new(1);

/// Probe the Docker registry using the exact public IPv4 address examined by the host.
pub(crate) async fn probe(request: ProbeEgress) -> EgressResult {
    let stage = match probe_stages(&request.address_v4).await {
        Ok(()) => EgressStage::Healthy,
        Err(stage) => stage,
    };
    EgressResult {
        stage: stage as i32,
    }
}

async fn probe_stages(bytes: &[u8]) -> Result<(), EgressStage> {
    let address = public_address(bytes).ok_or(EgressStage::Policy)?;
    let destination = SocketAddrV4::new(address, PORT);

    // Consult the local routing table before DNS. A UDP connect does not send a packet.
    timeout(ROUTE_LIMIT, check_route(destination))
        .await
        .map_err(|_| EgressStage::Policy)??;

    resolve(address).await?;

    let stream = timeout(CONNECT_LIMIT, TcpStream::connect(destination))
        .await
        .map_err(|_| EgressStage::Tcp)?
        .map_err(classify_connect_error)?;

    let connector = timeout(CA_LOAD_LIMIT, tls_connector())
        .await
        .map_err(|_| EgressStage::Tls)??;
    let name = ServerName::try_from(REGISTRY).map_err(|_| EgressStage::Tls)?;
    let stream = timeout(TLS_LIMIT, connector.connect(name, stream))
        .await
        .map_err(|_| EgressStage::Tls)?
        .map_err(|_| EgressStage::Tls)?;

    timeout(HTTP_LIMIT, registry_request(stream))
        .await
        .map_err(|_| EgressStage::Http)?
}

async fn resolve(address: Ipv4Addr) -> Result<(), EgressStage> {
    // The OS resolver may keep running after the deadline. Keep its permit in
    // the blocking task to prevent abandoned lookups from growing unbounded.
    let slot = RESOLVER.try_acquire().map_err(|_| EgressStage::Policy)?;
    let lookup = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        platform::matches_ipv4(DNS_NAME, address)
    });
    let found = timeout(LOOKUP_LIMIT, lookup)
        .await
        .map_err(|_| EgressStage::Dns)?
        .map_err(|_| EgressStage::Policy)?;
    if found {
        Ok(())
    } else {
        // libc cannot distinguish all resolver failures from policy replies.
        Err(EgressStage::Policy)
    }
}

fn public_address(bytes: &[u8]) -> Option<Ipv4Addr> {
    let octets: [u8; 4] = bytes.try_into().ok()?;
    // Refuse private, special-purpose, metadata and non-unicast destinations.
    if matches!(
        octets,
        [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [168, 63, 129, 16]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 0, 0 | 2, ..]
            | [192, 31, 196, ..]
            | [192, 52, 193, ..]
            | [192, 88, 99, ..]
            | [192, 175, 48, ..]
            | [192, 168, ..]
            | [198, 18 | 19, ..]
            | [198, 51, 100, ..]
            | [203, 0, 113, ..]
    ) {
        return None;
    }
    Some(octets.into())
}

async fn check_route(destination: SocketAddrV4) -> Result<(), EgressStage> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|_| EgressStage::Policy)?;
    socket.connect(destination).await.map_err(|error| {
        if no_route(&error) {
            EgressStage::Route
        } else {
            EgressStage::Policy
        }
    })?;
    match socket.local_addr() {
        Ok(SocketAddr::V4(local))
            if !local.ip().is_unspecified()
                && !local.ip().is_loopback()
                && !local.ip().is_link_local() =>
        {
            Ok(())
        }
        Ok(_) => Err(EgressStage::Route),
        Err(_) => Err(EgressStage::Policy),
    }
}

fn no_route(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NetworkDown
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::AddrNotAvailable
    )
}

fn classify_connect_error(error: io::Error) -> EgressStage {
    if no_route(&error) {
        return EgressStage::Route;
    }
    match error.kind() {
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::TimedOut => EgressStage::Tcp,
        _ => EgressStage::Policy,
    }
}

async fn tls_connector() -> Result<TlsConnector, EgressStage> {
    let mut bytes = Vec::new();
    tokio::fs::File::open(CA_BUNDLE)
        .await
        .map_err(|_| EgressStage::Tls)?
        .take(CA_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| EgressStage::Tls)?;
    if bytes.len() as u64 > CA_BYTES {
        return Err(EgressStage::Tls);
    }
    let certificates = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| EgressStage::Tls)?;
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(certificates);
    if roots.is_empty() {
        return Err(EgressStage::Tls);
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| EgressStage::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

async fn registry_request(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
) -> Result<(), EgressStage> {
    let (mut sender, connection) = http1::Builder::new()
        .max_buf_size(8192)
        .max_headers(32)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| EgressStage::Http)?;
    let request = Request::builder()
        .uri("/v2/")
        .header(HOST, REGISTRY)
        .header(CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| EgressStage::Http)?;
    // Drive the exchange in this task. If the connection closes in the same
    // poll as the reply arrives, collect the reply already sent to the callback.
    let mut response = std::pin::pin!(sender.send_request(request));
    let response = tokio::select! {
        biased;
        result = &mut response => result,
        _ = connection => response.await,
    }
    .map_err(|_| EgressStage::Http)?;
    if registry_replied(&response) {
        Ok(())
    } else {
        Err(EgressStage::Http)
    }
}

fn registry_replied<B>(response: &Response<B>) -> bool {
    let headers = response.headers();
    if !headers
        .get("docker-distribution-api-version")
        .is_some_and(|value| value == "registry/2.0")
    {
        return false;
    }
    match response.status() {
        StatusCode::OK => true,
        StatusCode::UNAUTHORIZED => headers.get_all(WWW_AUTHENTICATE).iter().any(|value| {
            value.to_str().ok().is_some_and(|challenge| {
                let mut parts = challenge.split_ascii_whitespace();
                parts
                    .next()
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("Bearer"))
                    && parts.next().is_some()
            })
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nonpublic_and_malformed_destinations() {
        for address in [
            [0, 0, 0, 0],
            [93, 184, 215, 14],
            [127, 0, 2, 1],
            [169, 254, 169, 254],
            [168, 63, 129, 16],
            [100, 100, 100, 200],
            [192, 168, 127, 1],
            [203, 0, 113, 12],
            [224, 1, 1, 1],
        ] {
            assert_eq!(
                public_address(&address),
                (address[0] == 93).then_some(address.into()),
                "{address:?}"
            );
        }
        assert_eq!(public_address(&[93, 184, 215]), None);
    }

    #[test]
    fn distinguishes_no_route_and_policy_from_tcp_failures() {
        for (kind, expected) in [
            (io::ErrorKind::NetworkUnreachable, EgressStage::Route),
            (io::ErrorKind::HostUnreachable, EgressStage::Route),
            (io::ErrorKind::PermissionDenied, EgressStage::Policy),
            (io::ErrorKind::ConnectionRefused, EgressStage::Tcp),
            (io::ErrorKind::TimedOut, EgressStage::Tcp),
        ] {
            assert_eq!(classify_connect_error(io::Error::from(kind)), expected);
        }
    }

    #[test]
    fn accepts_only_recognizable_registry_responses() {
        let response = |status, version: &str, challenge: &str| {
            Response::builder()
                .status(status)
                .header("docker-distribution-api-version", version)
                .header(WWW_AUTHENTICATE, challenge)
                .body(())
                .unwrap()
        };
        assert!(registry_replied(&response(200, "registry/2.0", "")));
        assert!(registry_replied(&response(
            401,
            "registry/2.0",
            "Bearer realm=\"https://auth.docker.io/token\""
        )));
        assert!(!registry_replied(&response(
            401,
            "registry/2.0",
            "Basic realm=portal"
        )));
        assert!(!registry_replied(&response(200, "", "")));
        assert!(!registry_replied(&response(302, "registry/2.0", "")));
    }

    #[tokio::test]
    async fn sends_fixed_request_and_ignores_response_body() {
        use tokio::io::AsyncWriteExt;

        let (client, mut peer) = tokio::io::duplex(8192);
        let serve = async {
            let mut request = [0; 1024];
            let count = peer.read(&mut request).await.unwrap();
            let request = std::str::from_utf8(&request[..count]).unwrap();
            assert!(request.starts_with("GET /v2/ HTTP/1.1\r\n"));
            assert!(request.contains("host: registry-1.docker.io\r\n"));
            peer.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nDocker-Distribution-Api-Version: registry/2.0\r\nWWW-Authenticate: Bearer realm=test\r\nContent-Length: 1000000\r\n\r\n",
            )
            .await
            .unwrap();
        };
        let (result, ()) = tokio::join!(registry_request(client), serve);
        assert_eq!(result, Ok(()));
    }
}
