//! Read the daemon's passive VM status stream while its dashboard is visible.

use marsa_client::{
    api::{VmStatsRequest, VmStatsSample},
    runtime::{Receiver, Task, queue},
};

use super::Services;

pub(crate) struct VmStats {
    samples: Receiver<VmStatsSample>,
    _worker: Task,
}

impl VmStats {
    pub(crate) async fn next(&mut self) -> Option<VmStatsSample> {
        self.samples.next().await
    }
}

impl Services {
    pub(crate) fn watch_vm(&self) -> VmStats {
        let endpoint = self.session.backend.daemon.clone();
        let (publish, samples) = queue(1);
        let worker = self.session.runtime.spawn(async move {
            let Ok(client) = endpoint.connect().await else {
                return;
            };
            let Ok(response) = client.stats().vm_stats(VmStatsRequest {}).await else {
                return;
            };
            let mut feed = response.into_inner();
            while let Ok(Some(sample)) = feed.message().await {
                if !publish.send(sample).await {
                    break;
                }
            }
        });
        VmStats {
            samples,
            _worker: worker,
        }
    }
}
