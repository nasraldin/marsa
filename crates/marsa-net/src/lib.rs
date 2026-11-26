//! Bound the resources consumed by host listeners and tunnel each flow into the guest.

use std::{future::Future, io, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

use marsa_core as _;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
    sync::{Semaphore, oneshot},
    task::{JoinHandle, JoinSet},
    time::timeout,
};

mod udp;

/// Transport of the Docker publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transport {
    /// A full-duplex byte stream.
    Tcp,
    /// Datagrams framed independently for each remote endpoint.
    Udp,
}

/// Bidirectional stream to the guest relay.
pub trait RelayStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RelayStream for T {}

/// Result of connecting a new transport to the active VM.
pub type Connection = Pin<Box<dyn Future<Output = io::Result<Box<dyn RelayStream>>> + Send>>;
/// Connects to the host-selected service inside the running VM.
pub type Connector = Arc<dyn Fn() -> Connection + Send + Sync>;

/// An active published port. Shutdown drains cancelled tasks before a replacement can bind.
pub struct Listener {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Listener {
    /// Bind exactly one address and family. Shared permits cap concurrent flows across listeners.
    pub fn bind(
        address: SocketAddr,
        protocol: Transport,
        connect: Connector,
        permits: Arc<Semaphore>,
    ) -> io::Result<Self> {
        let (kind, proto, number) = match protocol {
            Transport::Tcp => (Type::STREAM, Protocol::TCP, 6),
            Transport::Udp => (Type::DGRAM, Protocol::UDP, 17),
        };
        let socket = Socket::new(Domain::for_address(address), kind, Some(proto))?;
        if address.is_ipv6() {
            socket.set_only_v6(true)?;
        }
        if protocol == Transport::Tcp {
            socket.set_reuse_address(true)?;
        }
        socket.set_nonblocking(true)?;
        socket.bind(&address.into())?;
        let [hi, lo] = address.port().to_be_bytes();
        let header = [1, number, if address.is_ipv4() { 4 } else { 6 }, hi, lo];
        let (stop, stopped) = oneshot::channel();
        let task = match protocol {
            Transport::Tcp => {
                socket.listen(128)?;
                let socket = TcpListener::from_std(socket.into())?;
                tokio::spawn(tcp(socket, connect, permits, header, stopped))
            }
            Transport::Udp => {
                let socket = UdpSocket::from_std(socket.into())?;
                tokio::spawn(udp::listen(socket, connect, permits, header, stopped))
            }
        };
        Ok(Self { stop, task })
    }

    /// Close the listener and every accepted flow, including in-flight handshakes.
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

async fn connect_relay(connect: &Connector, header: [u8; 5]) -> io::Result<Box<dyn RelayStream>> {
    timeout(Duration::from_secs(5), async {
        let mut guest = connect().await?;
        guest.write_all(&header).await?;
        if guest.read_u8().await? != 0 {
            return Err(io::ErrorKind::ConnectionRefused.into());
        }
        Ok(guest)
    })
    .await?
}

async fn tcp(
    socket: TcpListener,
    connect: Connector,
    permits: Arc<Semaphore>,
    header: [u8; 5],
    mut stop: oneshot::Receiver<()>,
) {
    let mut flows = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => break,
            Some(_) = flows.join_next(), if !flows.is_empty() => (),
            result = socket.accept() => match result {
                Ok((mut client, _)) => {
                    let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                    let connect = connect.clone();
                    flows.spawn(async move {
                        let _permit = permit;
                        if let Ok(mut guest) = connect_relay(&connect, header).await {
                            let _ = tokio::io::copy_bidirectional(&mut client, &mut guest).await;
                        }
                    });
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    }
    flows.shutdown().await;
}

#[cfg(test)]
mod tests;
