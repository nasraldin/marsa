//! Resolve native compatibility aliases and reach a host service through stock Docker.

use super::{recovery, *};
use marsa_proto::host::v1::{NetworkStatusRequest, network_client::NetworkClient};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "requires a working IPv4 native NAT path on Apple silicon and a public registry"]
async fn native_aliases_reach_host_from_default_and_custom_bridges() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let log = scratch.path().join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    let mut settings = Settings::default();
    settings.disk.data_size_limit = GIB;
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Nat;
    replace_settings(&channel, &settings, 1).await;
    recovery::start(&channel, &paths, 2).await;
    let status = NetworkClient::new(channel.clone())
        .status(NetworkStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    let gateway: std::net::Ipv4Addr = status.gateway_address.unwrap().parse().unwrap();
    let guest: std::net::Ipv4Addr = status.guest_address.unwrap().parse().unwrap();
    // The Mac connects on the upstream NIC. Only guest-local Docker networks
    // may query the native DNS listener.
    assert!(!matches!(
        timeout(
            Duration::from_secs(1),
            tokio::net::TcpStream::connect((guest, 53))
        )
        .await,
        Ok(Ok(_))
    ));
    let udp = tokio::net::UdpSocket::bind((gateway, 0)).await.unwrap();
    udp.connect((guest, 53)).await.unwrap();
    udp.send(b"\xca\xfe\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x04host\x06docker\x08internal\x00\x00\x01\x00\x01").await.unwrap();
    let mut answer = [0; 512];
    assert!(!matches!(
        timeout(Duration::from_secs(1), udp.recv(&mut answer)).await,
        Ok(Ok(_))
    ));
    let listener = tokio::net::TcpListener::bind((gateway, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let service = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let mut size = 0;
            while !request[..size].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = timeout(WAIT, stream.read(&mut request[size..]))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(n > 0, "incomplete HTTP request");
                size += n;
            }
            assert!(request[..size].starts_with(b"GET /"));
            stream
                .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 8\r\n\r\nmarsa-ok")
                .await
                .unwrap();
        }
    });
    let sock = paths.docker_socket();
    let image = "busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec";
    recovery::docker(&sock, &["pull", image]).await;
    recovery::docker(&sock, &["network", "create", "qualification"]).await;
    // A length-prefixed A query with transaction ID cafe, sent over TCP.
    let query = "ACbK/gEAAAEAAAAAAAAEaG9zdAZkb2NrZXIIaW50ZXJuYWwAAAEAAQ==";
    for network in ["bridge", "qualification"] {
        let tcp = r#"resolver=$(awk '/^nameserver/{print $2; exit}' /etc/resolv.conf)
{ echo "$1" | base64 -d; sleep 1; } | nc -w 3 "$resolver" 53 | od -An -tx1"#;
        let reply = recovery::docker(
            &sock,
            &[
                "run",
                "--rm",
                "--network",
                network,
                image,
                "sh",
                "-c",
                tcp,
                "sh",
                query,
            ],
        )
        .await;
        let bytes: Vec<_> = reply
            .split_ascii_whitespace()
            .map(|s| u8::from_str_radix(s, 16).unwrap())
            .collect();
        assert!(bytes.len() > 14, "{network} missing TCP DNS reply: {reply}");
        assert_eq!(&bytes[2..4], &[0xca, 0xfe]);
        assert_eq!(bytes[5] & 0x0f, 0);
        assert!(u16::from_be_bytes([bytes[8], bytes[9]]) > 0);
        assert!(bytes.windows(4).any(|bytes| bytes == gateway.octets()));
        let script = "set -eu; nslookup registry-1.docker.io; for h in host.docker.internal gateway.docker.internal host.marsa.internal; do test \"$(wget -T 8 -qO- http://$h:$1/)\" = marsa-ok; done";
        recovery::docker(
            &sock,
            &[
                "run",
                "--rm",
                "--network",
                network,
                image,
                "sh",
                "-c",
                script,
                "sh",
                &port.to_string(),
            ],
        )
        .await;
    }
    service.abort();
    EngineClient::new(channel).stop(action(3)).await.unwrap();
    recovery::signal_running_daemon(&mut process).await;
}
