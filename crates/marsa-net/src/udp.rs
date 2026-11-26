//! Keep one guest relay per UDP peer, retaining message boundaries in both directions.

use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    sync::{Semaphore, mpsc, oneshot},
    task::JoinSet,
    time::timeout,
};

use super::{Connector, connect_relay};

const MAX_PACKET: usize = u16::MAX as usize;
const IDLE: Duration = Duration::from_secs(60);

pub(super) async fn listen(
    socket: UdpSocket,
    connect: Connector,
    permits: Arc<Semaphore>,
    header: [u8; 5],
    mut stop: oneshot::Receiver<()>,
) {
    let socket = Arc::new(socket);
    let mut peers: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut tasks = JoinSet::new();
    let mut buffer = vec![0; MAX_PACKET];
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => break,
            Some(Ok(peer)) = tasks.join_next(), if !tasks.is_empty() => { peers.remove(&peer); }
            result = socket.recv_from(&mut buffer) => {
                let Ok((size, peer)) = result else { continue };
                if let Some(tx) = peers.get(&peer) {
                    if let Ok(slot) = tx.try_reserve() {
                        slot.send(buffer[..size].to_vec());
                    }
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                let (tx, rx) = mpsc::channel(4);
                let _ = tx.try_send(buffer[..size].to_vec());
                peers.insert(peer, tx);
                let connect = connect.clone();
                let socket = socket.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let _ = relay(socket, peer, rx, connect, header).await;
                    peer
                });
            }
        }
    }
    tasks.shutdown().await;
}

async fn relay(
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    mut packets: mpsc::Receiver<Vec<u8>>,
    connect: Connector,
    header: [u8; 5],
) -> io::Result<()> {
    let (mut reader, mut writer) = tokio::io::split(connect_relay(&connect, header).await?);
    let outbound = async {
        while let Some(packet) = timeout(IDLE, packets.recv()).await? {
            // The receive buffer permits at most u16::MAX bytes.
            timeout(IDLE, async {
                writer.write_u16(packet.len() as u16).await?;
                writer.write_all(&packet).await
            })
            .await??;
        }
        Ok(())
    };
    let inbound = async {
        let mut packet = vec![0; MAX_PACKET];
        loop {
            let size = usize::from(reader.read_u16().await?);
            reader.read_exact(&mut packet[..size]).await?;
            socket.send_to(&packet[..size], peer).await?;
        }
    };
    tokio::select! {
        result = outbound => result,
        result = inbound => result,
    }
}
