//! Authorized local Unix clients are the only source of Docker connections.

mod hangup;

use std::{future::Future, io, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional},
    net::{UnixListener, UnixStream},
    task::JoinSet,
    time::timeout,
};

const CONNECTION_LIMIT: usize = 128;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Deadline and localized responses for opening the Docker engine.
#[derive(Clone, Copy)]
pub struct Startup {
    /// Maximum time to wait for the guest, including VM activation.
    pub timeout: Duration,
    /// Docker error message returned when the deadline expires.
    pub timed_out: &'static str,
    /// Docker error message returned when activation or connection fails.
    pub unavailable: &'static str,
}

/// Serve an owner-only Unix listener and open one upstream stream per authorized client.
///
/// The caller owns the socket path and its permissions. The opener should connect to the
/// active VM's Docker bridge. Dropping this future cancels every open or active connection.
pub async fn serve<Open, Fut, Stream>(
    listener: UnixListener,
    owner_uid: u32,
    startup: Startup,
    open: Open,
) -> io::Result<()>
where
    Open: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = io::Result<Stream>> + Send,
    Stream: AsyncRead + AsyncWrite + Unpin + Send,
{
    let open = Arc::new(open);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            incoming = listener.accept(), if connections.len() < CONNECTION_LIMIT => {
                let (client, _) = incoming?;
                if !(matches!(client.peer_cred(), Ok(peer) if peer.uid() == owner_uid)) {
                    continue;
                }
                let open = open.clone();
                connections.spawn(async move { connect_and_relay(client, startup, open).await });
            }
            Some(result) = connections.join_next() => match result {
                Ok(Ok(())) => (),
                Ok(Err(error)) => tracing::debug!(kind = ?error.kind(), "Docker connection closed"),
                Err(error) => tracing::warn!(%error, "Docker relay task failed"),
            },
        }
    }
}

async fn connect_and_relay<Open, Fut, Stream>(
    mut client: UnixStream,
    startup: Startup,
    open: Arc<Open>,
) -> io::Result<()>
where
    Open: Fn() -> Fut,
    Fut: Future<Output = io::Result<Stream>>,
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    let hangup = hangup::watch(&client)?;
    tokio::select! {
        biased;
        result = hangup::closed(&hangup) => result,
        result = async {
            let mut engine = match timeout(startup.timeout, open()).await {
                Ok(Ok(engine)) => engine,
                Ok(Err(error)) if error.kind() != io::ErrorKind::TimedOut => {
                    return startup_error(&mut client, "503 Service Unavailable", startup.unavailable).await;
                }
                Ok(Err(_)) | Err(_) => {
                    return startup_error(&mut client, "504 Gateway Timeout", startup.timed_out).await;
                }
            };
            // Tokio copies at most 8 KiB per direction at a time and propagates write EOF.
            copy_bidirectional(&mut client, &mut engine).await.map(|_| ())
        } => result,
    }
}

// Error responses are generated only when the upstream failed to open.
async fn startup_error(client: &mut UnixStream, status: &str, message: &str) -> io::Result<()> {
    timeout(RESPONSE_TIMEOUT, async {
        // Consume the request prefix so a HEAD response can omit its body.
        // The rest of the request is irrelevant once opening the engine failed.
        let mut prefix = [0; 4096];
        let mut length = 0;
        while length < 5 {
            let received = client.read(&mut prefix[length..]).await?;
            if received == 0 {
                return Ok(());
            }
            length += received;
        }
        let body = serde_json::json!({ "message": message }).to_string();
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        client.write_all(headers.as_bytes()).await?;
        if !prefix.starts_with(b"HEAD ") {
            client.write_all(body.as_bytes()).await?;
        }
        client.shutdown().await
    })
    .await?
}
