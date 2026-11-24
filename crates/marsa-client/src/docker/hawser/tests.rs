use std::{
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Stdio,
};

use hyper::{service::service_fn, upgrade};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::UnixListener,
    process::{Child, Command},
    task::JoinHandle,
};

use crate::docker::{
    ContainerId, ExecControl, ExecOutput, ManagedEndpointConfig, ManagedEndpointTransport,
    TerminalSize,
};

use super::*;

struct Fixture {
    directory: tempfile::TempDir,
    socket: PathBuf,
    _docker: super::super::stream::ConnectionTask,
}

impl Fixture {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("docker.sock");
        let docker = UnixListener::bind(&socket).unwrap();
        let worker: JoinHandle<()> = tokio::spawn(async move {
            while let Ok((stream, _)) = docker.accept().await {
                tokio::spawn(async move {
                    let _ = server::Builder::new()
                        .serve_connection(TokioIo::new(stream), service_fn(docker_fixture))
                        .with_upgrades()
                        .await;
                });
            }
        });
        let fixture = Self {
            directory,
            socket,
            _docker: super::super::stream::ConnectionTask::new(worker.abort_handle()),
        };
        fixture.certificates().await;
        fixture
    }

    fn path(&self, file: &str) -> PathBuf {
        self.directory.path().join(file)
    }

    async fn openssl(&self, args: &[&str]) {
        let output = Command::new("/usr/bin/openssl")
            .args(args)
            .current_dir(self.directory.path())
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "openssl generated invalid fixture material"
        );
    }

    async fn certificates(&self) {
        self.openssl(&[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=Marsa Hawser test CA",
            "-days",
            "1",
            "-keyout",
            "ca-key.pem",
            "-out",
            "ca.pem",
        ])
        .await;
        self.openssl(&[
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-keyout",
            "server-key.pem",
            "-out",
            "server.csr",
        ])
        .await;
        tokio::fs::write(self.path("extensions"), "basicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n").await.unwrap();
        self.openssl(&[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca-key.pem",
            "-CAcreateserial",
            "-days",
            "1",
            "-extfile",
            "extensions",
            "-out",
            "server.pem",
        ])
        .await;
        std::fs::set_permissions(
            self.path("server-key.pem"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    fn agent(&self, program: &Path, address: SocketAddr, token: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("DOCKER_SOCKET", &self.socket)
            .env("TOKEN", token)
            .env("AGENT_ID", "marsa-fixture")
            .env("BIND_ADDRESS", address.ip().to_string())
            .env("PORT", address.port().to_string())
            .env("HEARTBEAT_INTERVAL", "1")
            .env("MAX_RECONNECT_DELAY", "2")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    fn standard(&self, program: &Path, address: SocketAddr, token: &str) -> Child {
        self.agent(program, address, token)
            .env("TLS_CERT", self.path("server.pem"))
            .env("TLS_KEY", self.path("server-key.pem"))
            .spawn()
            .unwrap()
    }

    fn edge(&self, program: &Path, address: SocketAddr, token: &str) -> Child {
        self.agent(program, loopback_port(), token)
            .env(
                "DOCKHAND_SERVER_URL",
                format!("wss://{address}{HAWSER_EDGE_PATH}"),
            )
            .env("CA_CERT", self.path("ca.pem"))
            .spawn()
            .unwrap()
    }
}

async fn docker_fixture(
    mut request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let path = request.uri().path();
    if path.contains("/exec/") && path.ends_with("/start") {
        let upgraded = upgrade::on(&mut request);
        tokio::spawn(async move {
            let Ok(upgraded) = upgraded.await else { return };
            let mut stream = TokioIo::new(upgraded);
            if stream.write_all(b"fixture> ").await.is_err() {
                return;
            }
            let mut input = [0; 4];
            if stream.read_exact(&mut input).await.is_ok() {
                let _ = stream.write_all(&input).await;
            }
        });
        let mut response = Response::new(Full::new(Bytes::new()));
        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        response
            .headers_mut()
            .insert(header::UPGRADE, header::HeaderValue::from_static("tcp"));
        response.headers_mut().insert(
            header::CONNECTION,
            header::HeaderValue::from_static("Upgrade"),
        );
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/vnd.docker.raw-stream"),
        );
        return Ok(response);
    }
    let payload = if path.ends_with("/info") {
        r#"{"ID":"fixture-engine","Name":"fixture-node","ServerVersion":"29.10.0"}"#
    } else if path.ends_with("/version") {
        r#"{"Version":"29.10.0","ApiVersion":"1.54"}"#
    } else if path.ends_with("/_ping") {
        "OK"
    } else if path.ends_with("/exec") {
        r#"{"Id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#
    } else {
        "[]"
    };
    let mut response = Response::new(Full::new(Bytes::from_static(payload.as_bytes())));
    if path.ends_with("/exec") {
        *response.status_mut() = StatusCode::CREATED;
    }
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

async fn terminal_roundtrip(
    terminal: impl Future<Output = Result<super::super::ExecSession, super::super::DockerError>>,
) {
    let (input, controls) = crate::runtime::queue(4);
    let (output, mut records) = crate::runtime::queue(4);
    let terminal = terminal.await.unwrap();
    let worker = tokio::spawn(async move { terminal.drive(controls, &output).await });
    timeout(Duration::from_secs(5), async {
        assert_eq!(records.next().await.unwrap().unwrap(), ExecOutput::Opened);
        assert_eq!(
            records.next().await.unwrap().unwrap(),
            ExecOutput::Bytes(b"fixture> ".to_vec())
        );
        assert!(input.send(ExecControl::Input(b"ping".to_vec())).await);
        assert_eq!(
            records.next().await.unwrap().unwrap(),
            ExecOutput::Bytes(b"ping".to_vec())
        );
    })
    .await
    .unwrap();
    worker.abort();
}

fn loopback_port() -> SocketAddr {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
}

fn config(transport: ManagedEndpointTransport) -> ManagedEndpointConfig {
    ManagedEndpointConfig {
        icon: None,
        name: "Hawser fixture".into(),
        transport,
        labels: vec![],
        public_ip: None,
    }
}

fn upstream_binary() -> PathBuf {
    std::env::var_os("MARSA_TEST_HAWSER_BINARY")
        .map(PathBuf::from)
        .expect("set MARSA_TEST_HAWSER_BINARY to an upstream Hawser executable")
}

#[tokio::test]
#[ignore = "requires an upstream Hawser executable and OpenSSL"]
async fn real_standard_authenticates_both_tls_and_the_token() {
    let fixture = Fixture::start().await;
    let address = loopback_port();
    let token = crate::request_id().unwrap();
    let upstream = upstream_binary();
    let _agent = fixture.standard(&upstream, address, &token);
    timeout(Duration::from_secs(12), async {
        while TcpStream::connect(address).await.is_err() {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .unwrap();
    let settings = config(ManagedEndpointTransport::HawserStandard {
        address: format!("https://{address}"),
        ca: fixture.path("ca.pem"),
        credential: None,
    });
    let secret = token.clone();
    let engine = settings.test_with_hawser_token(Some(token)).await.unwrap();
    assert_eq!(engine.id, "fixture-engine");
    let upstream_endpoint = StandardEndpoint::load(
        &format!("https://{address}"),
        &fixture.path("ca.pem"),
        secret.clone(),
    )
    .await
    .unwrap();
    let endpoint = super::super::ExternalEndpoint::managed(
        "managed:standard-fixture".into(),
        super::super::inventory::ExternalTransport::HawserStandard(upstream_endpoint),
    );
    let container = ContainerId::parse("a".repeat(64)).unwrap();
    terminal_roundtrip(endpoint.start_terminal(&container, TerminalSize::new(80, 24).unwrap()))
        .await;
    assert!(
        settings
            .test_with_hawser_token(Some(crate::request_id().unwrap()))
            .await
            .is_err()
    );
    let wrong_hostname = config(ManagedEndpointTransport::HawserStandard {
        address: format!("https://127.0.0.2:{}", address.port()),
        ca: fixture.path("ca.pem"),
        credential: None,
    });
    assert!(
        wrong_hostname
            .test_with_hawser_token(Some(crate::request_id().unwrap()))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires an upstream Hawser executable and OpenSSL"]
async fn real_edge_rejects_mismatched_credentials_and_recovers_after_disconnect() {
    let fixture = Fixture::start().await;
    let address = loopback_port();
    let token = crate::request_id().unwrap();
    let cert = fixture.path("server.pem");
    let key = fixture.path("server-key.pem");
    let edge = EdgeEndpoint::listen(address, "marsa-fixture", &cert, &key, &token)
        .await
        .unwrap();
    let settings = config(ManagedEndpointTransport::HawserEdge {
        bind: address,
        cert,
        key,
        agent_id: "marsa-fixture".into(),
        credential: None,
    });
    let upstream = upstream_binary();
    let mut wrong_agent = fixture.edge(&upstream, address, &crate::request_id().unwrap());
    assert!(timeout(Duration::from_secs(3), edge.wait()).await.is_err());
    wrong_agent.kill().await.unwrap();

    let mut agent = fixture.edge(&upstream, address, &token);
    let engine = settings
        .test_with_hawser_token(Some(token.clone()))
        .await
        .unwrap();
    assert_eq!(engine.id, "fixture-engine");
    let endpoint = super::super::ExternalEndpoint::managed(
        "managed:edge-fixture".into(),
        super::super::inventory::ExternalTransport::HawserEdge(edge.clone()),
    );
    let container = ContainerId::parse("a".repeat(64)).unwrap();
    terminal_roundtrip(endpoint.start_terminal(&container, TerminalSize::new(80, 24).unwrap()))
        .await;
    let pinned_socket = edge.connect().unwrap();
    agent.kill().await.unwrap();

    timeout(Duration::from_secs(5), async {
        while edge.connect().is_ok() {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .unwrap();
    let _replacement = fixture.edge(&upstream, address, &token);
    assert!(settings.test_with_hawser_token(Some(token)).await.is_ok());

    let (mut sender, connection) = http1::handshake(TokioIo::new(pinned_socket)).await.unwrap();
    tokio::spawn(connection);
    let response = sender
        .send_request(Request::get("/info").body(Full::new(Bytes::new())).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "old transport never redirected to the replacement session"
    );
}
