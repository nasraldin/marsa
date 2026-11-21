//! Read actual guest network facts over the host-only virtio socket.

use std::{sync::Weak, time::Duration};

use marsa_proto::guest::{NETWORK_FACTS_BYTES, NETWORK_FACTS_PORT, decode_network_facts};
use marsa_vm::{NetworkInfo, VmState};
use tokio::{
    io::AsyncReadExt,
    sync::watch,
    time::{sleep, timeout},
};

use super::{native::Controller, vsock};

pub(super) async fn discover(controller: Weak<Controller>, mut state: watch::Receiver<VmState>) {
    loop {
        if state
            .wait_for(|state| *state == VmState::Running)
            .await
            .is_err()
        {
            return;
        }
        let Some(controller) = controller.upgrade() else {
            return;
        };
        let result = tokio::select! {
            _ = state.changed() => continue,
            result = timeout(Duration::from_secs(1), probe(&controller)) => result,
        };
        let observed = state.clone();
        let has_facts = controller
            .call("publish network facts", move |controller| {
                Ok(controller.publish_network(&observed, result.ok().flatten()))
            })
            .await
            .unwrap_or(false);
        let delay = if has_facts {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(100)
        };
        drop(controller);
        tokio::select! {
            change = state.changed() => {
                if change.is_err() {
                    return;
                }
            }
            _ = sleep(delay) => {}
        }
    }
}

async fn probe(controller: &std::sync::Arc<Controller>) -> Option<NetworkInfo> {
    let mut stream = vsock::connect(controller, NETWORK_FACTS_PORT).await.ok()?;
    let mut bytes = [0; NETWORK_FACTS_BYTES];
    stream.read_exact(&mut bytes).await.ok()?;
    let facts = decode_network_facts(bytes).ok()??;
    Some(NetworkInfo {
        address: facts.address.into(),
        gateway: facts.gateway.map(Into::into),
    })
}
