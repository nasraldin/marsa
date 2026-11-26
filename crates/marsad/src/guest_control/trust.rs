//! Synchronize replacement sets of public trust anchors with a single guest session.

use std::{io, sync::Arc, time::Duration};

use marsa_proto::guest::v1::{CommandStatus, InstallCertificates, host_envelope::Body};
use tokio::{
    sync::Semaphore,
    task::JoinHandle,
    time::{sleep, timeout},
};

use super::{
    GuestControlError, GuestResponse, HANDSHAKE_TIMEOUT, observations::SessionEvents,
    session::Client,
};

#[cfg(target_os = "macos")]
mod platform;
#[cfg(target_os = "macos")]
pub use platform::MacKeychainTrust;

const MAX_CERT_BYTES: usize = 16 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Return the complete set of public DER root certificates eligible for guest trust.
/// An empty snapshot revokes all previously imported anchors.
pub trait TrustSource: Send + Sync {
    /// Read trust settings without changing the host keychain or exporting keys.
    fn certificates(&self) -> io::Result<Vec<Vec<u8>>>;
}

#[derive(Clone)]
pub(super) struct Trust {
    source: Arc<dyn TrustSource>,
    read_slot: Arc<Semaphore>,
}

impl Trust {
    pub(super) fn new(source: impl TrustSource + 'static) -> Self {
        Self {
            source: Arc::new(source),
            read_slot: Arc::new(Semaphore::new(1)),
        }
    }

    async fn snapshot(&self) -> Vec<Vec<u8>> {
        let source = self.source.clone();
        let slot = self.read_slot.clone();
        let read = async move {
            let permit = slot.acquire_owned().await.map_err(io::Error::other)?;
            tokio::task::spawn_blocking(move || {
                // A timed-out Security.framework call retains its slot until it
                // actually returns; later refreshes cannot accumulate workers.
                let _permit = permit;
                bounded(source.certificates()?)
            })
            .await?
        };
        match timeout(Duration::from_secs(3), read).await {
            Ok(Ok(snapshot)) => snapshot,
            _ => {
                tracing::warn!("host trust settings unavailable; withdrawing imported roots");
                Vec::new()
            }
        }
    }

    pub(super) async fn provision(
        &self,
        client: &Arc<Client>,
        events: SessionEvents,
    ) -> Result<JoinHandle<()>, GuestControlError> {
        let mut installed = self.snapshot().await;
        let staged = apply(client, &installed).await?;
        if staged {
            events.trust_staged();
        }
        let trust = self.clone();
        let client = client.clone();
        Ok(tokio::spawn(async move {
            loop {
                sleep(REFRESH_INTERVAL).await;
                let next = trust.snapshot().await;
                if next == installed {
                    continue;
                }
                match apply(&client, &next).await {
                    Ok(restart) => {
                        installed = next;
                        if restart {
                            events.trust_staged();
                        }
                    }
                    Err(_) => tracing::warn!("guest trust refresh was not acknowledged"),
                }
            }
        }))
    }
}

fn bounded(mut roots: Vec<Vec<u8>>) -> io::Result<Vec<Vec<u8>>> {
    roots.sort_unstable();
    roots.dedup();
    if roots.len() > 128
        || roots
            .iter()
            .any(|root| root.is_empty() || root.len() > MAX_CERT_BYTES)
        || roots.iter().map(|root| root.len() + 4).sum::<usize>()
            > marsa_proto::guest::MAX_FRAME_BYTES - 64
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(roots)
}

async fn apply(client: &Client, roots: &[Vec<u8>]) -> Result<bool, GuestControlError> {
    let request = || {
        Body::InstallCertificates(InstallCertificates {
            certificates_der: roots.to_vec(),
        })
    };
    let result = client.request(request(), HANDSHAKE_TIMEOUT).await?;
    match result {
        GuestResponse::Command(result) if result.status() == CommandStatus::Ok => Ok(false),
        // Older guests use UNAVAILABLE for both a staged change and an
        // unsupported command. An acknowledged idempotent replay verifies the write.
        GuestResponse::Command(result) if result.status() == CommandStatus::Unavailable => {
            match client.request(request(), HANDSHAKE_TIMEOUT).await? {
                GuestResponse::Command(result) if result.status() == CommandStatus::Ok => Ok(true),
                _ => Err(GuestControlError::Sequence(
                    "guest did not confirm installed trust",
                )),
            }
        }
        _ => Err(GuestControlError::Sequence(
            "guest rejected trust configuration",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonicalizes_der_and_rejects_an_oversized_complete_snapshot() {
        assert_eq!(
            bounded(vec![vec![2], vec![1], vec![2]]).unwrap(),
            [vec![1], vec![2]]
        );
        assert!(bounded(vec![vec![7; MAX_CERT_BYTES + 1]]).is_err());
        assert!(bounded((0..128).map(|value| vec![value; MAX_CERT_BYTES]).collect()).is_err());
    }
}
