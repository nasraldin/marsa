//! Submit bind sources to the host on a bounded, sequential policy session.

use std::{io, time::Duration};

use marsa_proto::guest::{
    BIND_AUTHORIZATION_VERSION, bind_authorization_frame_length,
    decode_bind_authorization_response, encode_bind_authorization_request,
    v1::{BindAuthorizationDecision, BindAuthorizationRequest},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
    time::{Instant, timeout_at},
};

use super::{Decision, POLICY_UNAVAILABLE, QUERY_TOO_LARGE};

const QUEUE_CAPACITY: usize = 16;
const POLICY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
pub(crate) struct Policy {
    session: watch::Sender<Option<mpsc::Sender<Query>>>,
}

struct Query {
    sources: Vec<String>,
    deadline: Instant,
    reply: oneshot::Sender<Decision>,
}

impl Policy {
    pub(super) async fn authorize(&self, sources: Vec<String>) -> Decision {
        let sender = self.session.borrow().clone();
        let Some(sender) = sender else {
            return Decision::deny(POLICY_UNAVAILABLE);
        };
        let deadline = Instant::now() + POLICY_TIMEOUT;
        let (reply, receiver) = oneshot::channel();
        if sender
            .try_send(Query {
                sources,
                deadline,
                reply,
            })
            .is_err()
        {
            return Decision::deny(POLICY_UNAVAILABLE);
        }
        match timeout_at(deadline, receiver).await {
            Ok(Ok(decision)) => decision,
            _ => Decision::deny(POLICY_UNAVAILABLE),
        }
    }

    /// The platform listener calls this sequentially for accepted host-only streams.
    pub(crate) async fn serve(&self, mut stream: impl AsyncRead + AsyncWrite + Unpin) {
        let (sender, mut queue) = mpsc::channel(QUEUE_CAPACITY);
        self.session.send_replace(Some(sender));
        serve_session(&mut stream, &mut queue).await;
        self.session.send_replace(None);
    }
}

async fn serve_session(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    queue: &mut mpsc::Receiver<Query>,
) {
    let mut id = 1u64;
    loop {
        let mut unsolicited = [0];
        let query = tokio::select! {
            biased;
            // No host data is valid while no query is outstanding. This
            // also releases a terminated session without waiting for a bind.
            _ = stream.read(&mut unsolicited) => return,
            query = queue.recv() => query,
        };
        let Some(Query {
            sources,
            deadline,
            mut reply,
        }) = query
        else {
            return;
        };
        if reply.is_closed() || Instant::now() >= deadline {
            continue;
        }
        let Ok(frame) = encode_bind_authorization_request(&BindAuthorizationRequest {
            protocol: Some(BIND_AUTHORIZATION_VERSION),
            request_id: id,
            sources,
        }) else {
            let _ = reply.send(Decision::deny(QUERY_TOO_LARGE));
            continue;
        };
        let decision = tokio::select! {
            response = timeout_at(deadline, exchange(stream, id, &frame)) => response,
            // A partial exchange cannot be reused for another request.
            _ = reply.closed() => return,
        };
        let Ok(Ok(decision)) = decision else {
            return;
        };
        let _ = reply.send(decision);
        let Some(next) = id.checked_add(1) else {
            return;
        };
        id = next;
    }
}

async fn exchange(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    id: u64,
    frame: &[u8],
) -> io::Result<Decision> {
    stream.write_all(frame).await?;
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let length = bind_authorization_frame_length(header).map_err(io::Error::other)?;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await?;
    let response = decode_bind_authorization_response(&payload).map_err(io::Error::other)?;
    if response.request_id != id {
        return Err(io::Error::other("bind authorization response ID mismatch"));
    }
    Ok(match response.decision() {
        BindAuthorizationDecision::Allow => Decision::Allow,
        _ => Decision::Deny(response.message),
    })
}

#[cfg(test)]
mod tests {
    use marsa_proto::guest::{
        decode_bind_authorization_request, encode_bind_authorization_response,
        v1::BindAuthorizationResponse,
    };
    use tokio::io::DuplexStream;

    use super::*;

    async fn read_query(host: &mut DuplexStream) -> BindAuthorizationRequest {
        let mut header = [0; 4];
        host.read_exact(&mut header).await.unwrap();
        let mut payload = vec![0; bind_authorization_frame_length(header).unwrap()];
        host.read_exact(&mut payload).await.unwrap();
        decode_bind_authorization_request(&payload).unwrap()
    }

    async fn reply(
        host: &mut DuplexStream,
        id: u64,
        decision: BindAuthorizationDecision,
        message: &str,
    ) {
        let frame = encode_bind_authorization_response(&BindAuthorizationResponse {
            protocol: Some(BIND_AUTHORIZATION_VERSION),
            request_id: id,
            decision: decision.into(),
            message: message.into(),
        })
        .unwrap();
        host.write_all(&frame).await.unwrap();
    }

    async fn session(policy: &Policy) -> (DuplexStream, tokio::task::JoinHandle<()>) {
        let mut active = policy.session.subscribe();
        let (guest, host) = tokio::io::duplex(1024);
        let policy = policy.clone();
        let task = tokio::spawn(async move { policy.serve(guest).await });
        active.changed().await.unwrap();
        (host, task)
    }

    #[tokio::test]
    async fn sends_only_sources_and_uses_sequential_correlated_responses() {
        let policy = Policy::default();
        let (mut host, task) = session(&policy).await;
        let first = policy.authorize(vec!["/allowed".into()]);
        let second = policy.authorize(vec!["/denied".into()]);
        let remote = async {
            let request = read_query(&mut host).await;
            assert_eq!(request.request_id, 1);
            assert_eq!(request.sources, ["/allowed"]);
            reply(&mut host, 1, BindAuthorizationDecision::Allow, "").await;
            let request = read_query(&mut host).await;
            assert_eq!(request.request_id, 2);
            assert_eq!(request.sources, ["/denied"]);
            reply(
                &mut host,
                2,
                BindAuthorizationDecision::Deny,
                "denied by host",
            )
            .await;
        };
        let (first, second, ()) = tokio::join!(first, second, remote);
        assert_eq!(first, Decision::Allow);
        assert_eq!(second, Decision::deny("denied by host"));
        drop(host);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn drops_mismatched_transport_and_starts_over_on_reconnect() {
        let policy = Policy::default();
        let (mut host, task) = session(&policy).await;
        let remote = async {
            assert_eq!(read_query(&mut host).await.request_id, 1);
            reply(&mut host, 2, BindAuthorizationDecision::Allow, "").await;
        };
        let (decision, ()) = tokio::join!(policy.authorize(vec!["/request".into()]), remote);
        assert_eq!(decision, Decision::deny(POLICY_UNAVAILABLE));
        task.await.unwrap();

        let (mut host, task) = session(&policy).await;
        let remote = async {
            assert_eq!(read_query(&mut host).await.request_id, 1);
            reply(&mut host, 1, BindAuthorizationDecision::Allow, "").await;
        };
        let (decision, ()) = tokio::join!(policy.authorize(vec!["/request".into()]), remote);
        assert_eq!(decision, Decision::Allow);
        drop(host);
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn times_out_unanswered_requests_and_closes_the_session() {
        let policy = Policy::default();
        let (mut host, task) = session(&policy).await;
        let pending = async {
            assert_eq!(read_query(&mut host).await.request_id, 1);
            tokio::time::sleep(POLICY_TIMEOUT).await;
        };
        let (decision, ()) = tokio::join!(policy.authorize(vec!["/request".into()]), pending);
        assert_eq!(decision, Decision::deny(POLICY_UNAVAILABLE));
        task.await.unwrap();
        assert_eq!(host.read(&mut [0]).await.unwrap(), 0);
    }
}
