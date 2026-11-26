//! Verify the signed daemon's Docker publications from host loopback and a second VM.
//! The independent `tart_peer` test needs a running macOS Tart guest with Python 3
//! named by MARSA_TEST_TART_PEER. Set MARSA_TEST_PEER_HOST_IPV4 and
//! MARSA_TEST_PEER_HOST_IPV6 to non-loopback Mac addresses reachable from that VM.

use super::*;
use marsa_core::settings::NetworkMode as Mode;
use marsa_proto::host::v1::{
    ListForwardedPortsRequest, ListForwardedPortsResponse, PortTransport,
    network_client::NetworkClient,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};

// Linux/arm64 manifests for the official nginx:stable-alpine and python:3.13-alpine images.
const NGINX: &str = "nginx@sha256:4bd7abff67b3541b9f2ff905196b1f29d204c167c238af6f05f49f2fb061788f";
const PYTHON: &str =
    "python@sha256:9cf2fdca7b3dd6082fd160ff5b8664f108c7441e200018a36554b2e8028f45df";
const ECHO: &str = r#"import socket,sys
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(('', 9000))
prefix = sys.argv[1].encode()
print('ready', flush=True)
while True:
    data, peer = sock.recvfrom(1024)
    if data.startswith(prefix):
        sock.sendto(data, peer)
"#;
const V4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const V6: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);

struct Fixture {
    process: Daemon,
    channel: Channel,
    socket: PathBuf,
    nonce: String,
    config: Settings,
    _scratch: tempfile::TempDir,
}

impl Fixture {
    async fn boot(mode: Mode) -> Self {
        let scratch = tempfile::tempdir_in("/tmp").unwrap();
        let home = scratch.path().join("home");
        fs::create_dir(&home).unwrap();
        let paths = MarsaPaths::from_home(&home).unwrap();
        let (daemon, _) = stage_bundle(scratch.path());
        let log = scratch.path().join("daemon.log");
        let mut process = recovery::spawn(&daemon, &home, &log);
        let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
        let mut config = Settings::default();
        config.disk.data_size_limit = GIB * 2;
        config.vm.memory = Some(1024 * MIB);
        config.vm.idle_stop = false;
        config.network.mode = mode;
        assert!(!config.network.allow_lan, "LAN requires explicit opt-in");
        replace_settings(&channel, &config, 1).await;
        recovery::start(&channel, &paths, 2).await;
        let nonce = format!(
            "marsa-{}",
            scratch.path().file_name().unwrap().to_str().unwrap()
        );
        Self {
            process,
            channel,
            socket: paths.docker_socket(),
            nonce,
            config,
            _scratch: scratch,
        }
    }

    async fn docker(&self, args: &[&str]) -> String {
        recovery::docker(&self.socket, args)
            .await
            .trim()
            .to_string()
    }

    async fn publish(&self, transport: PortTransport, mapping: &str) -> Publication {
        let (id, guest_port) = match transport {
            PortTransport::Tcp => {
                let start =
                    r#"printf %s "$1" > "/usr/share/nginx/html/$1" && exec nginx -g 'daemon off;'"#;
                (
                    self.docker(&[
                        "run",
                        "-d",
                        "-p",
                        mapping,
                        NGINX,
                        "sh",
                        "-c",
                        start,
                        "sh",
                        &self.nonce,
                    ])
                    .await,
                    "80/tcp",
                )
            }
            PortTransport::Udp => (
                self.docker(&[
                    "run",
                    "-d",
                    "-p",
                    mapping,
                    PYTHON,
                    "python3",
                    "-c",
                    ECHO,
                    &self.nonce,
                ])
                .await,
                "9000/udp",
            ),
            _ => unreachable!(),
        };
        if transport == PortTransport::Udp {
            timeout(WAIT, async {
                while self.docker(&["logs", &id]).await != "ready" {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .expect("UDP echo did not bind in the container");
        }
        let published = self.docker(&["port", &id, guest_port]).await;
        let host: SocketAddr = published.lines().next().unwrap().parse().unwrap();
        Publication {
            id,
            port: host.port(),
            transport,
        }
    }

    async fn wait_list(&self, matches: impl Fn(&ListForwardedPortsResponse) -> bool) {
        let mut api = NetworkClient::new(self.channel.clone());
        let deadline = Instant::now() + WAIT;
        loop {
            let current = timeout(WAIT, api.list_forwarded_ports(ListForwardedPortsRequest {}))
                .await
                .unwrap()
                .unwrap()
                .into_inner();
            if matches(&current) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "unexpected forwarding inventory: {current:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn ready(&self, p: &Publication, addresses: &[&str]) {
        self.wait_list(|list| {
            addresses.iter().all(|addr| {
                list.ports.iter().any(|row| {
                    row.container_id == p.id
                        && row.transport() == p.transport
                        && row.host_port == u32::from(p.port)
                        && row.host_address == *addr
                })
            })
        })
        .await;
    }

    async fn remove(&self, p: &Publication) {
        self.docker(&["rm", "-f", &p.id]).await;
        self.wait_list(|list| {
            !list.ports.iter().any(|row| row.container_id == p.id)
                && !list.failures.iter().any(|row| row.container_id == p.id)
        })
        .await;
    }

    async fn stop(mut self) {
        EngineClient::new(self.channel.clone())
            .stop(action(3))
            .await
            .unwrap();
        self.wait_list(|list| list.ports.is_empty() && list.failures.is_empty())
            .await;
        recovery::signal_running_daemon(&mut self.process).await;
    }
}

struct Publication {
    id: String,
    port: u16,
    transport: PortTransport,
}

async fn http(address: IpAddr, port: u16, nonce: &str) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((address, port)).await?;
    let request = format!("GET /{nonce} HTTP/1.0\r\nHost: localhost\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut reply = String::new();
    stream.take(16384).read_to_string(&mut reply).await?;
    if !reply.starts_with("HTTP/1.")
        || !reply.lines().next().unwrap_or("").contains(" 200 ")
        || reply.split_once("\r\n\r\n").map(|(_, body)| body) != Some(nonce)
    {
        return Err(std::io::Error::other(format!(
            "unexpected HTTP reply: {reply}"
        )));
    }
    Ok(())
}

async fn traffic(f: &Fixture, tcp: &Publication, udp: &Publication, address: IpAddr) {
    timeout(WAIT, async {
        // Publication can precede nginx startup.
        loop {
            if http(address, tcp.port, &f.nonce).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("nginx did not serve its nonce");
    let mut clients = Vec::new();
    for index in 0..4 {
        let client = UdpSocket::bind((address, 0)).await.unwrap();
        client.connect((address, udp.port)).await.unwrap();
        clients.push((client, format!("{}-flow-{index}", f.nonce)));
    }
    timeout(WAIT, async {
        // Keep all four source ports open and interleave datagrams before reading any replies.
        for round in 0..2 {
            for (client, label) in &clients {
                client
                    .send(format!("{label}-{round}").as_bytes())
                    .await
                    .unwrap();
            }
            for (client, label) in clients.iter().rev() {
                let mut bytes = [0; 1024];
                let len = client.recv(&mut bytes).await.unwrap();
                assert_eq!(&bytes[..len], format!("{label}-{round}").as_bytes());
            }
        }
    })
    .await
    .expect("concurrent UDP echo did not complete");
}

async fn local(mode: Mode) {
    let f = Fixture::boot(mode).await;
    let tcp = f.publish(PortTransport::Tcp, "80/tcp").await;
    let udp = f.publish(PortTransport::Udp, "9000/udp").await;
    let both = ["127.0.0.1", "::1"];
    f.ready(&tcp, &both).await;
    f.ready(&udp, &both).await;
    for address in [V4, V6] {
        traffic(&f, &tcp, &udp, address).await;
    }

    let occupied_tcp = std::net::TcpListener::bind((V4, 0)).unwrap();
    let occupied_udp = std::net::UdpSocket::bind((V4, 0)).unwrap();
    for (transport, port, guest) in [
        (
            PortTransport::Tcp,
            occupied_tcp.local_addr().unwrap().port(),
            "80/tcp",
        ),
        (
            PortTransport::Udp,
            occupied_udp.local_addr().unwrap().port(),
            "9000/udp",
        ),
    ] {
        let conflict = f
            .publish(transport, &format!("127.0.0.1:{port}:{guest}"))
            .await;
        let pid = format!("({})", std::process::id());
        f.wait_list(|list| {
            list.failures.iter().any(|row| {
                row.container_id == conflict.id
                    && row.host_port == u32::from(port)
                    && row.host_address == "127.0.0.1"
                    && row.transport() == transport
                    && row.message.contains(&port.to_string())
                    && row
                        .owner
                        .as_ref()
                        .is_some_and(|owner| owner.ends_with(&pid))
            }) && !list.ports.iter().any(|row| row.container_id == conflict.id)
        })
        .await;
        f.remove(&conflict).await;
    }

    for p in [&tcp, &udp] {
        f.remove(p).await;
    }
    for address in [V4, V6] {
        assert!(TcpStream::connect((address, tcp.port)).await.is_err());
        UdpSocket::bind((address, udp.port))
            .await
            .expect("UDP port still held after teardown");
    }
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires the signed VZ fixture, Docker CLI and registry access"]
async fn local_nat() {
    local(Mode::Nat).await;
}

#[tokio::test]
#[ignore = "requires the signed VZ fixture, Docker CLI and registry access"]
async fn local_userspace() {
    local(Mode::Userspace).await;
}

struct Peer {
    name: String,
    hosts: [IpAddr; 2],
}

impl Peer {
    fn from_env() -> Self {
        let name = env::var("MARSA_TEST_TART_PEER").expect("set MARSA_TEST_TART_PEER");
        let v4: Ipv4Addr = env::var("MARSA_TEST_PEER_HOST_IPV4")
            .expect("set MARSA_TEST_PEER_HOST_IPV4")
            .parse()
            .unwrap();
        let v6: Ipv6Addr = env::var("MARSA_TEST_PEER_HOST_IPV6")
            .expect("set MARSA_TEST_PEER_HOST_IPV6")
            .parse()
            .unwrap();
        let hosts = [IpAddr::V4(v4), IpAddr::V6(v6)];
        assert!(
            hosts
                .iter()
                .all(|ip| !ip.is_loopback() && !ip.is_unspecified())
        );
        Self { name, hosts }
    }

    async fn check(&self, f: &Fixture, p: &Publication, expected: &str) {
        let protocol = match p.transport {
            PortTransport::Tcp => "tcp",
            PortTransport::Udp => "udp",
            _ => unreachable!(),
        };
        for host in self.hosts {
            let output = timeout(
                WAIT,
                tokio::process::Command::new("tart")
                    .args([
                        "exec",
                        &self.name,
                        "/usr/bin/python3",
                        "-c",
                        include_str!("forwarding_peer.py"),
                        &host.to_string(),
                        &p.port.to_string(),
                        protocol,
                        &f.nonce,
                        expected,
                    ])
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                output.status.success(),
                "{protocol} {host}:{} expected {expected}: {}",
                p.port,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires the two-machine Tart fixture described in forwarding.rs"]
async fn tart_peer() {
    let peer = Peer::from_env();
    for mode in [Mode::Nat, Mode::Userspace] {
        let mut f = Fixture::boot(mode).await;
        let tcp = f.publish(PortTransport::Tcp, "80/tcp").await;
        let udp = f.publish(PortTransport::Udp, "9000/udp").await;
        let fixed_tcp = f.publish(PortTransport::Tcp, "127.0.0.1::80/tcp").await;
        let fixed_udp = f.publish(PortTransport::Udp, "127.0.0.1::9000/udp").await;
        for (step, allow_lan) in [(0, false), (4, true), (5, false)] {
            if step != 0 {
                f.config.network.allow_lan = allow_lan;
                replace_settings(&f.channel, &f.config, step).await;
            }
            let (addresses, expected) = if allow_lan {
                (["0.0.0.0", "::"], "reachable")
            } else {
                (["127.0.0.1", "::1"], "blocked")
            };
            for p in [&tcp, &udp] {
                f.ready(p, &addresses).await;
            }
            for address in [V4, V6] {
                traffic(&f, &tcp, &udp, address).await;
            }
            for p in [&tcp, &udp] {
                peer.check(&f, p, expected).await;
            }
            for p in [&fixed_tcp, &fixed_udp] {
                f.ready(p, &["127.0.0.1"]).await;
            }
            traffic(&f, &fixed_tcp, &fixed_udp, V4).await;
            for p in [&fixed_tcp, &fixed_udp] {
                peer.check(&f, p, "blocked").await;
            }
        }
        // Leave these publications live so the stop assertion exercises engine teardown.
        f.stop().await;
    }
}
