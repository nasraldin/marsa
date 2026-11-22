//! Exercise the client over actual Unix connections without launching a guest VM.

use std::{cell::Cell, fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use marsa_proto::host::{API_MAJOR, API_MINOR};
use tokio::{
    net::UnixListener,
    sync::mpsc,
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tonic::{Request, Response, Status, transport::Server};

use super::{ClientError, DaemonEndpoint, api::*, platform};

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap()
}

fn bind_private_socket(path: &Path) -> UnixListener {
    let listener = UnixListener::bind(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    listener
}

struct TestDaemon {
    serving: JoinHandle<()>,
    events: mpsc::Sender<Result<DaemonEvent, Status>>,
}

impl TestDaemon {
    fn start(path: &Path, major: u32) -> Self {
        Self::serve(bind_private_socket(path), major)
    }

    fn serve(listener: UnixListener, major: u32) -> Self {
        Self::serve_with_stall(listener, major, None)
    }

    fn serve_with_stall(
        listener: UnixListener,
        major: u32,
        stall: Option<(Stall, mpsc::Sender<()>)>,
    ) -> Self {
        let (events, receiver) = mpsc::channel(8);
        let fixture = Fixture {
            major,
            events: std::sync::Mutex::new(Some(receiver)),
            stall,
            first_subscribe: std::sync::atomic::AtomicBool::new(true),
        };
        let serving = tokio::spawn(async move {
            Server::builder()
                .add_service(meta_server::MetaServer::new(MetaVersion(major)))
                .add_service(events_server::EventsServer::new(fixture))
                .serve_with_incoming(UnixListenerStream::new(listener))
                .await
                .unwrap();
        });
        Self { serving, events }
    }

    async fn send(&self, id: &str, sequence: u64, snapshot: bool) {
        let body = if snapshot {
            daemon_event::Body::Snapshot(DaemonSnapshot {
                settings_revision: sequence,
                ..Default::default()
            })
        } else {
            daemon_event::Body::Settings(SettingsChanged { revision: sequence })
        };
        self.events
            .send(Ok(DaemonEvent {
                stream_id: id.into(),
                sequence,
                body: Some(body),
                ..Default::default()
            }))
            .await
            .unwrap();
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

struct MetaVersion(u32);

#[tonic::async_trait]
impl meta_server::Meta for MetaVersion {
    async fn version(
        &self,
        _: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        Ok(Response::new(VersionResponse {
            api_major: self.0,
            api_minor: API_MINOR,
            daemon_version: "fixture".into(),
            ..Default::default()
        }))
    }

    async fn capabilities(
        &self,
        _: Request<CapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        Ok(Response::new(CapabilitiesResponse::default()))
    }
}

#[derive(Clone, Copy)]
enum Stall {
    Subscribe,
    Snapshot,
}

struct Fixture {
    first_subscribe: std::sync::atomic::AtomicBool,
    stall: Option<(Stall, mpsc::Sender<()>)>,
    major: u32,
    events: std::sync::Mutex<Option<mpsc::Receiver<Result<DaemonEvent, Status>>>>,
}

#[tonic::async_trait]
impl events_server::Events for Fixture {
    type SubscribeStream = ReceiverStream<Result<DaemonEvent, Status>>;

    async fn subscribe(
        &self,
        _: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        assert_eq!(self.major, API_MAJOR);
        if let Some((stall, attempts)) = &self.stall {
            attempts.send(()).await.unwrap();
            if !self
                .first_subscribe
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                return Err(Status::unavailable("fixture is restarting again"));
            }
            if matches!(stall, Stall::Subscribe) {
                return std::future::pending().await;
            }
        }
        let receiver = self
            .events
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Status::unavailable("fixture has no new snapshot"))?;
        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

#[tokio::test]
async fn starts_once_and_connects_when_daemon_is_ready() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let endpoint = DaemonEndpoint::at(socket.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let connecting = endpoint.connect_with(
        || async {
            tokio::spawn(async move {
                sleep(Duration::from_millis(25)).await;
                assert!(sender.send(TestDaemon::start(&socket, API_MAJOR)).is_ok());
            });
            Ok(())
        },
        platform::open_socket,
    );
    let client = timeout(Duration::from_secs(3), connecting)
        .await
        .unwrap()
        .unwrap();
    let _fixture = receiver.await.unwrap();
    assert_eq!(client.version().daemon_version, "fixture");
    client
        .meta()
        .capabilities(CapabilitiesRequest {})
        .await
        .unwrap();
}

#[tokio::test]
async fn event_stream_replaces_old_state_with_new_generation_then_continues() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let daemon = TestDaemon::start(&socket, API_MAJOR);
    let client = DaemonEndpoint::at(socket.clone()).connect().await.unwrap();
    let mut events = client.subscribe_events();
    daemon.send("first", 2, true).await;
    daemon.send("first", 3, false).await;
    assert_eq!(events.next().await.unwrap().sequence, 2);
    assert_eq!(events.next().await.unwrap().sequence, 3);
    drop(daemon);
    fs::remove_file(&socket).unwrap();
    let daemon = TestDaemon::start(&socket, API_MAJOR);
    daemon.send("second", 0, true).await;
    daemon.send("second", 1, false).await;
    let snapshot = timeout(Duration::from_secs(3), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (snapshot.stream_id.as_str(), snapshot.sequence),
        ("second", 0)
    );
    assert!(matches!(
        snapshot.body,
        Some(daemon_event::Body::Snapshot(_))
    ));
    let live = events.next().await.unwrap();
    assert_eq!((live.stream_id.as_str(), live.sequence), ("second", 1));
}

#[tokio::test]
async fn incompatible_daemon_version_is_rejected() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let _daemon = TestDaemon::start(&socket, API_MAJOR + 1);
    let result = DaemonEndpoint::at(socket).connect().await;
    assert!(
        matches!(result, Err(ClientError::IncompatibleVersion { daemon, supported })
        if daemon == API_MAJOR + 1 && supported == API_MAJOR)
    );
}

#[tokio::test]
async fn same_cursor_snapshot_is_suppressed_after_reconnect() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let daemon = TestDaemon::start(&socket, API_MAJOR);
    let client = DaemonEndpoint::at(socket.clone()).connect().await.unwrap();
    let mut events = client.subscribe_events();
    daemon.send("same", 3, true).await;
    assert_eq!(events.next().await.unwrap().sequence, 3);
    drop(daemon);
    fs::remove_file(&socket).unwrap();
    let daemon = TestDaemon::start(&socket, API_MAJOR);
    daemon.send("same", 3, true).await;
    daemon.send("same", 4, false).await;
    let event = timeout(Duration::from_secs(3), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.sequence, 4);
    assert!(matches!(event.body, Some(daemon_event::Body::Settings(_))));
}

#[tokio::test]
async fn out_of_order_live_event_is_rejected() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let daemon = TestDaemon::start(&socket, API_MAJOR);
    let client = DaemonEndpoint::at(socket).connect().await.unwrap();
    let mut events = client.subscribe_events();
    daemon.send("daemon", 2, true).await;
    daemon.send("daemon", 4, false).await;
    assert_eq!(events.next().await.unwrap().sequence, 2);
    assert!(matches!(
        events.next().await,
        Err(ClientError::EventProtocol(_))
    ));
}

#[tokio::test]
async fn socket_symlink_is_rejected_without_starting_daemon() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    std::os::unix::fs::symlink(directory.path().join("missing"), &socket).unwrap();
    let endpoint = DaemonEndpoint::at(socket);
    let result = endpoint
        .connect_with(
            || async { panic!("must not launch") },
            platform::open_socket,
        )
        .await;
    assert!(matches!(result, Err(ClientError::Socket(ref error))
        if error.kind() == std::io::ErrorKind::PermissionDenied));
}

#[tokio::test]
async fn reconnects_when_first_peer_drops_during_transport_setup() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let listener = bind_private_socket(&socket);
    let closed = std::sync::Arc::new(tokio::sync::Notify::new());
    let peer_closed = closed.clone();
    let starting = tokio::spawn(async move {
        let (initial, _) = listener.accept().await.unwrap();
        drop(initial);
        peer_closed.notify_one();
        TestDaemon::serve(listener, API_MAJOR)
    });
    let first = Cell::new(true);
    let endpoint = DaemonEndpoint::at(socket);
    let result = timeout(
        Duration::from_secs(3),
        endpoint.connect_with(
            || async { panic!("a connected endpoint should not need a launch") },
            |path| {
                let first = &first;
                let closed = &closed;
                async move {
                    let socket = platform::open_socket(path).await?;
                    if first.replace(false) {
                        // Make the peer drop deterministic between Unix authentication and HTTP/2.
                        closed.notified().await;
                    }
                    Ok(socket)
                }
            },
        ),
    )
    .await;
    assert_eq!(result.unwrap().unwrap().version().daemon_version, "fixture");
    let _daemon = starting.await.unwrap();
}

async fn resumes_after_restart_during_stalled_setup(stall: Stall) {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let before = TestDaemon::start(&socket, API_MAJOR);
    before.send("before", 2, true).await;
    let client = DaemonEndpoint::at(socket.clone()).connect().await.unwrap();
    let mut subscription = client.subscribe_events();
    assert_eq!(subscription.next().await.unwrap().stream_id, "before");

    drop(before);
    fs::remove_file(&socket).unwrap();
    let (send, mut attempts) = mpsc::channel(2);
    let stalled =
        TestDaemon::serve_with_stall(bind_private_socket(&socket), API_MAJOR, Some((stall, send)));
    let next = tokio::spawn(async move { subscription.next().await });
    // The first replacement accepts the connection but stalls during setup.
    // Wait for the client to exhaust that attempt's deadline and try again.
    timeout(Duration::from_secs(8), async {
        for _ in 0..2 {
            attempts.recv().await.unwrap();
        }
    })
    .await
    .unwrap();

    drop(stalled);
    fs::remove_file(&socket).unwrap();
    let after = TestDaemon::start(&socket, API_MAJOR);
    after.send("after", 0, true).await;
    let snapshot = timeout(Duration::from_secs(3), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        (snapshot.stream_id.as_str(), snapshot.sequence),
        ("after", 0)
    );
    assert!(matches!(
        snapshot.body,
        Some(daemon_event::Body::Snapshot(_))
    ));
}

#[tokio::test]
async fn resubscribes_after_a_restart_stalls_rpc_headers() {
    resumes_after_restart_during_stalled_setup(Stall::Subscribe).await;
}

#[tokio::test]
async fn resubscribes_after_a_restart_stalls_initial_snapshot() {
    resumes_after_restart_during_stalled_setup(Stall::Snapshot).await;
}

#[tokio::test]
async fn event_reconnect_rejects_an_incompatible_replacement_daemon() {
    let directory = private_directory();
    let socket = directory.path().join("marsad.sock");
    let before = TestDaemon::start(&socket, API_MAJOR);
    before.send("before", 2, true).await;
    let client = DaemonEndpoint::at(socket.clone()).connect().await.unwrap();
    let mut subscription = client.subscribe_events();
    assert_eq!(subscription.next().await.unwrap().stream_id, "before");
    drop(before);
    fs::remove_file(&socket).unwrap();

    let _after = TestDaemon::start(&socket, API_MAJOR + 1);
    let result = timeout(Duration::from_secs(3), subscription.next())
        .await
        .unwrap();
    assert!(
        matches!(result, Err(ClientError::IncompatibleVersion { daemon, .. }) if daemon == API_MAJOR + 1)
    );
}
