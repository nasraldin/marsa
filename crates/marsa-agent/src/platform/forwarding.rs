//! Carry a published Docker port over host-only vsock without trusting a guest
//! address to select a host or LAN destination. Each UDP stream serves one peer.

use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional},
    net::{TcpStream, UdpSocket},
    task::JoinSet,
    time::{sleep, timeout},
};

use super::vsock::Listener;

const LIMIT: usize = 256;
const CONNECT: Duration = Duration::from_secs(5);

pub(super) async fn serve(listener: Listener) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            stream = listener.accept_host(), if connections.len() < LIMIT => match stream {
                Ok(stream) => { connections.spawn(relay(stream)); }
                Err(_) => sleep(Duration::from_millis(100)).await,
            },
            Some(_) = connections.join_next() => (),
        }
    }
}

async fn relay(mut stream: impl AsyncRead + AsyncWrite + Unpin) -> io::Result<()> {
    let mut header = [0; 5];
    timeout(CONNECT, stream.read_exact(&mut header)).await??;
    let [version, protocol, family, hi, lo] = header;
    let port = u16::from_be_bytes([hi, lo]);
    let ip = match family {
        4 => IpAddr::V4(Ipv4Addr::LOCALHOST),
        6 => IpAddr::V6(Ipv6Addr::LOCALHOST),
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    if version != 1 || port == 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let address = SocketAddr::new(ip, port);
    match protocol {
        6 => {
            let connected = timeout(CONNECT, TcpStream::connect(address)).await?;
            let mut peer = match connected {
                Ok(peer) => peer,
                Err(error) => {
                    stream.write_u8(1).await?;
                    return Err(error);
                }
            };
            stream.write_u8(0).await?;
            copy_bidirectional(&mut stream, &mut peer).await?;
            Ok(())
        }
        17 => {
            let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).await?;
            socket.connect(address).await?;
            stream.write_u8(0).await?;
            let (mut incoming, mut outgoing) = tokio::io::split(stream);
            // Each loop owns its frame until completion, so concurrent messages
            // in the other direction cannot cancel a partially read length.
            let to_guest = async {
                let mut packet = vec![0; u16::MAX.into()];
                loop {
                    let length = usize::from(incoming.read_u16().await?);
                    incoming.read_exact(&mut packet[..length]).await?;
                    socket.send(&packet[..length]).await?;
                }
            };
            let to_host = async {
                let mut packet = vec![0; u16::MAX.into()];
                loop {
                    let length = socket.recv(&mut packet).await?;
                    outgoing.write_u16(length as u16).await?;
                    outgoing.write_all(&packet[..length]).await?;
                }
            };
            tokio::select! { result = to_guest => result, result = to_host => result }
        }
        _ => Err(io::ErrorKind::InvalidInput.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn udp_preserves_datagrams_in_both_directions() {
        let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = peer.local_addr().unwrap().port().to_be_bytes();
        let (guest, mut host) = duplex(128);
        let relay = tokio::spawn(relay(guest));
        host.write_all(&[1, 17, 4, port[0], port[1]]).await.unwrap();
        assert_eq!(host.read_u8().await.unwrap(), 0);
        // Deliberately fragment a frame, then send an empty datagram.
        host.write_all(&[0]).await.unwrap();
        host.write_all(&[3, 1, 2, 3, 0, 0]).await.unwrap();
        let mut packet = [0; 8];
        let (n, address) = peer.recv_from(&mut packet).await.unwrap();
        assert_eq!(&packet[..n], &[1, 2, 3]);
        assert_eq!(peer.recv_from(&mut packet).await.unwrap(), (0, address));
        peer.send_to(&[9, 8], address).await.unwrap();
        peer.send_to(&[], address).await.unwrap();
        let mut reply = [0; 6];
        host.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0, 2, 9, 8, 0, 0]);
        drop(host);
        assert!(relay.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_version_and_zero_port() {
        for header in [
            [2, 6, 4, 0, 80],
            [1, 6, 4, 0, 0],
            [1, 1, 4, 0, 80],
            [1, 6, 3, 0, 80],
        ] {
            let (guest, mut host) = duplex(16);
            host.write_all(&header).await.unwrap();
            assert_eq!(
                relay(guest).await.unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
