//! Fake clients exercise the same worker-to-view boundary as the desktop.

use std::{
    future::pending,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use marsa_client::{
    api::{DaemonSnapshot, EngineStatus},
    docker::{LogSource, VolumeSummary},
};

use super::*;

const WAIT: Duration = Duration::from_secs(15);

struct Logs {
    next: usize,
    end: usize,
    drop_notice: mpsc::Sender<&'static str>,
}

impl Drop for Logs {
    fn drop(&mut self) {
        let _ = self.drop_notice.send("logs");
    }
}

impl Feed<LogLine> for Logs {
    async fn pull(&mut self) -> Result<Option<LogLine>, ServiceError> {
        assert_eq!(thread::current().name(), Some("marsa-client"));
        if self.next == self.end {
            return Ok(None);
        }
        let text = self.next.to_string();
        self.next += 1;
        Ok(Some(LogLine {
            source: LogSource::Stdout,
            text,
            timestamp: None,
        }))
    }
}

struct OpenFeed {
    name: &'static str,
    ready: mpsc::Sender<&'static str>,
    dropped: mpsc::Sender<&'static str>,
}

impl Drop for OpenFeed {
    fn drop(&mut self) {
        let _ = self.dropped.send(self.name);
    }
}

impl<T: Send + 'static> Feed<T> for OpenFeed {
    async fn pull(&mut self) -> Result<Option<T>, ServiceError> {
        self.ready.send(self.name).unwrap();
        pending().await
    }
}

struct DaemonFeed {
    lifetime: OpenFeed,
    events: Option<Receiver<DaemonEvent>>,
}

impl Feed<DaemonEvent> for DaemonFeed {
    async fn pull(&mut self) -> Result<Option<DaemonEvent>, ServiceError> {
        if let Some(events) = &mut self.events {
            Ok(events.next().await)
        } else {
            self.lifetime.pull().await
        }
    }
}

struct Fake {
    opens: mpsc::Sender<&'static str>,
    closes: mpsc::Sender<&'static str>,
    log_count: usize,
    events: Mutex<Option<Receiver<DaemonEvent>>>,
    snapshots: Option<Mutex<Receiver<ResourceSnapshot>>>,
    reads: Option<mpsc::Sender<bool>>,
}

impl Fake {
    fn open(&self, name: &'static str) -> OpenFeed {
        OpenFeed {
            name,
            ready: self.opens.clone(),
            dropped: self.closes.clone(),
        }
    }
}

impl Clients for Fake {
    type Daemon = DaemonFeed;
    type Logs = Logs;
    type Stats = OpenFeed;

    async fn daemon(&self) -> Result<Self::Daemon, ServiceError> {
        Ok(DaemonFeed {
            lifetime: self.open("daemon"),
            events: self.events.lock().unwrap().take(),
        })
    }

    async fn inventory(&self, prefer_cache: bool) -> Result<ResourceSnapshot, ServiceError> {
        if let Some(reads) = &self.reads {
            reads.send(prefer_cache).unwrap();
        }
        Ok(self
            .snapshots
            .as_ref()
            .map_or_else(ResourceSnapshot::default, |snapshots| {
                snapshots.lock().unwrap().try_next().unwrap()
            }))
    }

    async fn inspect(&self, _id: &ContainerId) -> Result<ContainerDetails, ServiceError> {
        unreachable!()
    }

    async fn logs(&self, _id: &ContainerId) -> Result<Self::Logs, ServiceError> {
        self.opens.send("logs").unwrap();
        Ok(Logs {
            next: 0,
            end: self.log_count,
            drop_notice: self.closes.clone(),
        })
    }

    async fn stats(&self, _id: &ContainerId) -> Result<Self::Stats, ServiceError> {
        Ok(self.open("stats"))
    }

    async fn dispatch(&self, _command: Command) -> Result<(), ServiceError> {
        Ok(())
    }
}

fn id() -> ContainerId {
    ContainerId::parse("a".repeat(64)).unwrap()
}

#[test]
fn delivers_a_burst_in_bounded_batches_in_order() {
    let (opens, _opened) = mpsc::channel();
    let (closes, _closed) = mpsc::channel();
    let session = Session::start(
        Fake {
            opens,
            closes,
            log_count: 100_000,
            events: Mutex::default(),
            snapshots: None,
            reads: None,
        },
        true,
    )
    .unwrap();
    let mut logs = session.logs(id());
    let (send, received) = mpsc::channel();
    let _view = session.runtime.spawn(async move {
        while let Some(batch) = logs.next().await {
            send.send(batch.unwrap()).unwrap();
        }
    });
    let mut lines = 0;
    while let Ok(batch) = received.recv_timeout(WAIT) {
        assert!(!batch.is_empty() && batch.len() <= LOG_LINES);
        assert!(batch.iter().map(|line| line.text.len()).sum::<usize>() <= LOG_BYTES);
        for line in batch {
            assert_eq!(line.text, lines.to_string());
            lines += 1;
        }
    }
    assert_eq!(lines, 100_000);
}

#[test]
fn closing_a_window_cancels_feeds_even_with_live_view_handles() {
    let (opens, opened) = mpsc::channel();
    let (closes, closed) = mpsc::channel();
    // Leave the log producer blocked by backpressure with a live reader.
    let session = Arc::new(
        Session::start(
            Fake {
                opens,
                closes,
                log_count: usize::MAX,
                events: Mutex::default(),
                snapshots: None,
                reads: None,
            },
            true,
        )
        .unwrap(),
    );
    let retained_session = Arc::clone(&session);
    let _logs = session.logs(id());
    let _stats = session.stats(id(), None);
    let read_names = |receiver: mpsc::Receiver<_>| {
        let mut result = [
            receiver.recv_timeout(WAIT).unwrap(),
            receiver.recv_timeout(WAIT).unwrap(),
            receiver.recv_timeout(WAIT).unwrap(),
        ];
        result.sort_unstable();
        result
    };
    let expected = ["daemon", "logs", "stats"];
    assert_eq!(read_names(opened), expected);

    session.close();
    drop(session);
    assert_eq!(read_names(closed), expected);
    drop(retained_session);
}

#[test]
fn reads_cached_then_live_inventory_and_refetches_on_daemon_invalidation() {
    let (opens, _opened) = mpsc::channel();
    let (closes, _closed) = mpsc::channel();
    let (read, reads) = mpsc::channel();
    let (event_tx, events) = queue(4);
    let (new_snapshot, snapshots) = queue(4);
    let session = Session::start(
        Fake {
            opens,
            closes,
            log_count: 0,
            events: Mutex::new(Some(events)),
            snapshots: Some(Mutex::new(snapshots)),
            reads: Some(read),
        },
        true,
    )
    .unwrap();
    let mut volumes = session.store.select(|state| state.volumes());
    let (publish, rendered) = mpsc::channel();
    let _view = session.runtime.spawn(async move {
        while let Some(volumes) = volumes.changed().await {
            publish
                .send(volumes.keys().cloned().collect::<Vec<_>>())
                .unwrap();
        }
    });

    let event = |body| DaemonEvent {
        stream_id: "engine".into(),
        body: Some(body),
        ..Default::default()
    };
    let status = |state: EngineState| EngineStatus {
        state: state.into(),
        ..Default::default()
    };
    let inventory = |name: &str| ResourceSnapshot {
        volumes: vec![VolumeSummary {
            name: name.into(),
            driver: "local".into(),
            scope: "local".into(),
            created_at: None,
        }],
        ..Default::default()
    };
    assert!(new_snapshot.try_send(inventory("cached")));
    assert!(new_snapshot.try_send(inventory("live")));
    assert!(event_tx.try_send(event(Body::Snapshot(DaemonSnapshot {
        engine: Some(status(EngineState::Ready)),
        ..Default::default()
    }))));
    assert!(reads.recv_timeout(WAIT).unwrap());
    assert!(!reads.recv_timeout(WAIT).unwrap());
    // Watch projections can legitimately coalesce the cached frame with the first live frame.
    let mut first = rendered.recv_timeout(WAIT).unwrap();
    if first == ["cached"] {
        first = rendered.recv_timeout(WAIT).unwrap();
    }
    assert_eq!(first, ["live"]);

    assert!(new_snapshot.try_send(inventory("changed")));
    assert!(event_tx.try_send(event(Body::DockerInventoryChanged(Default::default()))));
    assert!(!reads.recv_timeout(WAIT).unwrap());
    assert_eq!(rendered.recv_timeout(WAIT).unwrap(), ["changed"]);

    assert!(event_tx.try_send(event(Body::Engine(status(EngineState::Stopped)))));
    assert!(rendered.recv_timeout(WAIT).unwrap().is_empty());
    assert!(event_tx.try_send(event(Body::DockerInventoryChanged(Default::default()))));
    assert!(reads.recv_timeout(Duration::from_millis(40)).is_err());
}

#[test]
fn limits_stats_delivery_to_the_latest_sample_in_each_batch() {
    struct Samples(std::ops::Range<usize>);

    impl Feed<StatsSample> for Samples {
        async fn pull(&mut self) -> Result<Option<StatsSample>, ServiceError> {
            Ok(self.0.next().map(|index| StatsSample {
                read: Some(index.to_string()),
                memory_stats: Default::default(),
                cpu_stats: Default::default(),
                precpu_stats: Default::default(),
                networks: Default::default(),
                blkio_stats: Default::default(),
                pids_stats: Default::default(),
            }))
        }
    }

    let runtime = Background::new().unwrap();
    let mut batches = stats_batches(&runtime, async { Ok(Samples(0..100)) });
    let (publish, received) = mpsc::channel();
    let _view = runtime.spawn(async move {
        while let Some(batch) = batches.next().await {
            publish.send(batch.unwrap()).unwrap();
        }
    });
    let mut latest = Vec::new();
    while let Ok(batch) = received.recv_timeout(WAIT) {
        assert_eq!(batch.len(), 1);
        latest.extend(
            batch
                .into_iter()
                .map(|sample| sample.read.unwrap().parse::<usize>().unwrap()),
        );
    }
    assert!(latest.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(latest.last(), Some(&99));
    assert!(latest.len() < 100);
}
