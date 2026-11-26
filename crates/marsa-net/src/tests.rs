//! Exercise the host data path against a fragmented in-memory guest transport.

use super::*;
use tokio::net::TcpStream;

fn address(ip: &str) -> SocketAddr {
    std::net::TcpListener::bind((ip, 0))
        .unwrap()
        .local_addr()
        .unwrap()
}

fn echo(expected: [u8; 5]) -> Connector {
    Arc::new(move || {
        Box::pin(async move {
            let (host, mut guest) = tokio::io::duplex(7);
            tokio::spawn(async move {
                let mut actual = [0; 5];
                guest.read_exact(&mut actual).await.unwrap();
                assert_eq!(actual, expected);
                guest.write_u8(0).await.unwrap();
                let (mut reader, mut writer) = tokio::io::split(guest);
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
            Ok(Box::new(host) as Box<dyn RelayStream>)
        })
    })
}

#[tokio::test]
async fn tcp_relays_both_families_and_shutdown_closes_active_clients() {
    for (ip, family) in [("127.0.0.1", 4), ("::1", 6)] {
        let address = address(ip);
        let [hi, lo] = address.port().to_be_bytes();
        let permits = Arc::new(Semaphore::new(2));
        let listener = Listener::bind(
            address,
            Transport::Tcp,
            echo([1, 6, family, hi, lo]),
            permits.clone(),
        )
        .unwrap();
        let mut clients = Vec::new();
        for message in [b"first", b"other"] {
            let mut client = TcpStream::connect(address).await.unwrap();
            client.write_all(message).await.unwrap();
            let mut reply = [0; 5];
            timeout(Duration::from_secs(2), client.read_exact(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&reply, message);
            clients.push(client);
        }
        assert_eq!(permits.available_permits(), 0);
        let mut excess = TcpStream::connect(address).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), excess.read(&mut [0]))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        listener.shutdown().await;
        for mut client in clients {
            assert_eq!(client.read(&mut [0]).await.unwrap(), 0);
        }
        assert_eq!(permits.available_permits(), 2);
        assert!(TcpStream::connect(address).await.is_err());
    }
}

#[tokio::test]
async fn udp_keeps_empty_and_fragmented_datagrams_separate_for_each_peer() {
    for (ip, family) in [("127.0.0.1", 4), ("::1", 6)] {
        let address = UdpSocket::bind((ip, 0))
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let [hi, lo] = address.port().to_be_bytes();
        let permits = Arc::new(Semaphore::new(2));
        let listener = Listener::bind(
            address,
            Transport::Udp,
            echo([1, 17, family, hi, lo]),
            permits.clone(),
        )
        .unwrap();
        let first = UdpSocket::bind((ip, 0)).await.unwrap();
        let second = UdpSocket::bind((ip, 0)).await.unwrap();
        let payload = vec![0x4b; 1201];
        for (socket, packet) in [
            (&first, payload.as_slice()),
            (&second, &[]),
            (&first, b"next"),
        ] {
            socket.send_to(packet, address).await.unwrap();
        }
        for (socket, packet) in [
            (&second, &[][..]),
            (&first, payload.as_slice()),
            (&first, b"next"),
        ] {
            let mut reply = [0; 1400];
            let (size, sender) = timeout(Duration::from_secs(2), socket.recv_from(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(sender, address);
            assert_eq!(&reply[..size], packet);
        }
        assert_eq!(permits.available_permits(), 0);
        listener.shutdown().await;
        assert_eq!(permits.available_permits(), 2);
        UdpSocket::bind(address).await.unwrap();
    }
}
