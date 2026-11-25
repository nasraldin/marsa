//! Observe Docker changes without acquiring a public Docker socket activity lease.

use std::{io, time::Duration};

use http_body_util::BodyExt as _;
use hyper::body::Incoming;
use serde::Deserialize;
use tokio::time::{MissedTickBehavior, interval, sleep};

use crate::{
    guest_control::docker_http,
    supervisor::{Snapshot, State, Supervisor},
};

use super::{TypedApi, events::Bus};

// Replay Docker's bounded history to cover the gap around opening the stream.
const EVENT_PATH: &str = "/v1.57/events?since=1&filters=%7B%22type%22%3A%5B%22container%22%2C%22image%22%2C%22volume%22%2C%22network%22%5D%7D";
const EVENT_BYTES: usize = 64 * 1024;
const BATCH_INTERVAL: Duration = Duration::from_millis(250);
const RETRY: Duration = Duration::from_secs(1);

impl TypedApi {
    pub(super) fn spawn_inventory_observer(&self) {
        if let Some(supervisor) = &self.supervisor {
            let supervisor = supervisor.clone();
            let bus = self.inner.events.clone();
            let mut closing = self.inner.closing.subscribe();
            tokio::spawn(async move {
                if !*closing.borrow_and_update() {
                    tokio::select! {
                        _ = closing.changed() => {}
                        () = watch(&supervisor, &bus) => {}
                    }
                }
            });
        }
    }
}

async fn watch(supervisor: &Supervisor, bus: &Bus) {
    let mut snapshots = supervisor.snapshots();
    loop {
        if snapshots.wait_for(ready).await.is_err() {
            return;
        }
        tokio::select! {
            closed = async { snapshots.wait_for(|snapshot| !ready(snapshot)).await.is_err() } => {
                if closed {
                    return;
                }
            }
            result = follow(supervisor, bus) => {
                if let Err(error) = result {
                    tracing::debug!(%error, "Docker inventory observation interrupted");
                }
                sleep(RETRY).await;
            }
        }
    }
}

fn ready(snapshot: &Snapshot) -> bool {
    matches!(snapshot.state, State::Ready | State::Idle)
}

async fn follow(supervisor: &Supervisor, bus: &Bus) -> io::Result<()> {
    let stream = supervisor.connect_running_docker().await?;
    docker_http::get_with(stream, EVENT_PATH, |body| relay(body, bus)).await
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerEvent {
    #[serde(rename = "Type")]
    kind: String,
    action: String,
}

impl DockerEvent {
    fn changes_inventory(&self) -> bool {
        match self.kind.as_str() {
            "container" => {
                matches!(
                    self.action.as_str(),
                    "create"
                        | "destroy"
                        | "die"
                        | "pause"
                        | "rename"
                        | "restart"
                        | "start"
                        | "stop"
                        | "unpause"
                        | "update"
                        | "prune"
                ) || self.action.starts_with("health_status:")
            }
            "image" => !matches!(self.action.as_str(), "push" | "save"),
            "volume" | "network" => true,
            _ => false,
        }
    }
}

async fn relay(mut body: Incoming, bus: &Bus) -> io::Result<()> {
    // Attaching a new stream may follow a gap, so schedule a fresh snapshot.
    let mut changed = true;
    let mut ticks = interval(BATCH_INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut records = EventRecords::default();
    loop {
        tokio::select! {
            _ = ticks.tick(), if changed => {
                bus.invalidate_docker_inventory();
                changed = false;
            }
            frame = body.frame() => {
                let Some(frame) = frame else {
                    if changed {
                        bus.invalidate_docker_inventory();
                    }
                    return Ok(());
                };
                let Ok(data) = frame.map_err(io::Error::other)?.into_data() else {
                    continue;
                };
                changed |= records.push(&data);
            }
        }
    }
}

#[derive(Default)]
struct EventRecords {
    line: Vec<u8>,
    skipping: bool,
}

impl EventRecords {
    fn push(&mut self, bytes: &[u8]) -> bool {
        let mut changed = false;
        for fragment in bytes.split_inclusive(|byte| *byte == b'\n') {
            if self.line.len().saturating_add(fragment.len()) > EVENT_BYTES {
                self.line.clear();
                self.skipping = true;
            }
            if !self.skipping {
                self.line.extend_from_slice(fragment);
            }
            if fragment.ends_with(b"\n") {
                if !self.skipping
                    && let Ok(event) = serde_json::from_slice::<DockerEvent>(&self.line)
                {
                    changed |= event.changes_inventory();
                }
                self.skipping = false;
                self.line.clear();
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_records_skip_unsafe_lines_and_resume_at_a_newline() {
        let mut records = EventRecords::default();
        assert!(!records.push(b"{\"Type\":\"container\",\"Action\":"));
        assert!(records.push(b"\"health_status: healthy\"}\n"));
        assert!(!records.push(b"{\"Type\":\"container\",\"Action\":\"exec_start: sh\"}\n"));

        assert!(!records.push(&vec![b'x'; EVENT_BYTES]));
        assert!(
            records.push(b"truncated\ninvalid\n{\"Type\":\"network\",\"Action\":\"connect\"}\n")
        );
        assert!(records.line.is_empty());
    }
}
