//! Authorized local Unix clients are the only source of Docker connections.

mod hangup;

use std::{future::Future, io, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncWrite, copy_bidirectional},
    net::{UnixListener, UnixStream},
    task::JoinSet,
    time::timeout,
};

const CONNECTION_LIMIT: usize = 128;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Serve an owner-only Unix listener and open one upstream stream per authorized client.
///
/// The caller owns the socket path and its permissions. The opener should connect to the
/// active VM's Docker bridge. Dropping this future cancels every open or active connection.
pub async fn serve<Open, Fut, Stream>(
    listener: UnixListener,
    owner_uid: u32,
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
                connections.spawn(async move { connect_and_relay(client, open).await });
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
            let mut engine = timeout(CONNECT_TIMEOUT, open()).await??;
            // Tokio copies at most 8 KiB per direction at a time and propagates write EOF.
            copy_bidirectional(&mut client, &mut engine).await.map(|_| ())
        } => result,
    }
}
