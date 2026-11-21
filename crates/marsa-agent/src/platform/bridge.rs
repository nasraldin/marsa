//! Pass Docker bytes unchanged, including streamed and upgraded connections.

use std::io;

use tokio::{
    io::{AsyncRead, AsyncWrite, copy_bidirectional},
    net::UnixStream,
    task::JoinSet,
    time::{Duration, sleep},
};

use super::vsock::Listener;

const CONNECTION_LIMIT: usize = 128;

pub(super) async fn serve(listener: &Listener, engine_socket: &str) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            incoming = listener.accept_host(), if connections.len() < CONNECTION_LIMIT => {
                match incoming {
                    Ok(mut stream) => {
                        let path = engine_socket.to_owned();
                        connections.spawn(async move {
                            if let Ok(mut engine) = UnixStream::connect(path).await {
                                let _ = relay(&mut stream, &mut engine).await;
                            }
                        });
                    }
                    Err(error) => {
                        eprintln!("marsa-agent: accept Docker API connection: {error}");
                        sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            Some(result) = connections.join_next() => {
                if let Err(error) = result {
                    eprintln!("marsa-agent: Docker API bridge task: {error}");
                }
            }
        }
    }
}

async fn relay(
    client: &mut (impl AsyncRead + AsyncWrite + Unpin),
    engine: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> io::Result<()> {
    copy_bidirectional(client, engine).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    use super::relay;

    #[tokio::test]
    async fn relays_upgrade_and_binary_payload_after_client_half_close() {
        let (mut host, mut client) = duplex(32);
        let (mut engine, mut docker) = duplex(32);
        let request = b"POST /exec/test/start HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n\x00\xff";
        let response =
            b"HTTP/1.1 101 UPGRADED\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n\xff\x00";
        let guest = async {
            let mut bytes = Vec::new();
            docker.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, request);
            docker.write_all(response).await.unwrap();
            docker.shutdown().await.unwrap();
        };
        let caller = async {
            host.write_all(request).await.unwrap();
            host.shutdown().await.unwrap();
            let mut bytes = Vec::new();
            host.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, response);
        };
        let (result, (), ()) = tokio::join!(relay(&mut client, &mut engine), guest, caller);
        result.unwrap();
    }
}
