//! Exercise the daemon through its process and production Unix gRPC socket.

#![cfg(unix)]

use std::{
    fs, io,
    os::unix::fs::{FileTypeExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use hyper_util::rt::TokioIo;
use marsa_proto::host::{
    API_MAJOR, API_MINOR,
    v1::{
        CapabilitiesRequest, EngineActionRequest, EngineState, EngineStatusRequest,
        GetSettingsRequest, SubscribeRequest, UpdateSettingsRequest, VersionRequest,
        daemon_event::Body, engine_client::EngineClient, events_client::EventsClient,
        meta_client::MetaClient, settings_client::SettingsClient,
    },
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tempfile::TempDir;
use tokio::{net::UnixStream, time::sleep};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn serves_meta_and_cleans_up_its_runtime_files_on_sigterm() {
    let home = TempDir::new_in("/tmp").unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::write(paths.config_file(), "schema_version = 1\n[vm]\ncpus = 4\n").unwrap();
    let mut daemon = Daemon(start(home.path()).spawn().unwrap());
    let mut client = wait_for_meta(&mut daemon, paths.daemon_socket()).await;
    let version = client
        .version(VersionRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        (version.api_major, version.api_minor),
        (API_MAJOR, API_MINOR)
    );
    assert_eq!(version.daemon_version, env!("CARGO_PKG_VERSION"));
    assert!(
        client
            .capabilities(CapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner()
            .backends
            .is_empty()
    );
    assert_eq!(mode(paths.run_dir()), 0o700);
    assert_eq!(mode(paths.daemon_socket()), 0o600);
    assert!(
        fs::symlink_metadata(paths.daemon_socket())
            .unwrap()
            .file_type()
            .is_socket()
    );
    assert_eq!(mode(paths.daemon_pid_file()), 0o600);
    assert_eq!(
        fs::read_to_string(paths.daemon_pid_file()).unwrap().trim(),
        daemon.0.id().to_string()
    );

    let second = start(home.path()).output().unwrap();
    assert!(!second.status.success());
    assert!(
        String::from_utf8(second.stderr)
            .unwrap()
            .contains("already running")
    );
    assert!(client.version(VersionRequest {}).await.is_ok());

    daemon.terminate().await;
    assert!(!paths.daemon_socket().exists());
    assert!(!paths.daemon_pid_file().exists());
    assert!(paths.daemon_lock_file().is_file());
    let log = fs::read_to_string(paths.daemon_log()).unwrap();
    let events: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["fields"]["message"] == "daemon started")
    );
    assert!(
        events
            .iter()
            .any(|event| event["fields"]["message"] == "daemon stopped")
    );
}

#[tokio::test]
async fn recovers_stale_socket_and_pid_after_an_unclean_exit() {
    let home = TempDir::new_in("/tmp").unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    let mut first = Daemon(start(home.path()).spawn().unwrap());
    wait_for_meta(&mut first, paths.daemon_socket()).await;
    first.0.kill().unwrap();
    first.0.wait().unwrap();
    assert!(paths.daemon_socket().exists());
    assert!(paths.daemon_pid_file().exists());

    let mut replacement = Daemon(start(home.path()).spawn().unwrap());
    let mut client = wait_for_meta(&mut replacement, paths.daemon_socket()).await;
    assert!(client.version(VersionRequest {}).await.is_ok());
    assert_eq!(
        fs::read_to_string(paths.daemon_pid_file()).unwrap().trim(),
        replacement.0.id().to_string()
    );
    replacement.terminate().await;
}

#[test]
fn refuses_a_symlink_runtime_directory_and_invalid_settings() {
    let home = TempDir::new_in("/tmp").unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    fs::create_dir(paths.root()).unwrap();
    let outside = home.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, paths.run_dir()).unwrap();
    let rejected = start(home.path()).output().unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("private directory")
    );
    assert!(fs::symlink_metadata(paths.run_dir()).unwrap().is_symlink());
    assert!(fs::read_dir(outside).unwrap().next().is_none());

    fs::remove_file(paths.run_dir()).unwrap();
    fs::write(paths.config_file(), "schema_version = 999\n").unwrap();
    let invalid = start(home.path()).output().unwrap();
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("settings")
    );
    assert!(!paths.daemon_socket().exists());
    assert!(!paths.daemon_pid_file().exists());
}

#[test]
fn leaves_an_unexpected_socket_path_and_its_target_intact() {
    let home = TempDir::new_in("/tmp").unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    fs::create_dir_all(paths.run_dir()).unwrap();
    let sentinel = home.path().join("sentinel");
    fs::write(&sentinel, "keep this file").unwrap();
    symlink(&sentinel, paths.daemon_socket()).unwrap();
    let rejected = start(home.path()).output().unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("unexpected owner or file type")
    );
    assert_eq!(fs::read_link(paths.daemon_socket()).unwrap(), sentinel);
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep this file");
    assert!(!paths.daemon_pid_file().exists());
}

#[tokio::test]
async fn rotates_the_daily_json_logs_and_keeps_the_current_path() {
    let home = TempDir::new_in("/tmp").unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    fs::create_dir_all(paths.logs_dir()).unwrap();
    for day in 1..=12 {
        fs::write(
            paths
                .logs_dir()
                .join(format!("marsad.2000-01-{day:02}.jsonl")),
            "{}\n",
        )
        .unwrap();
    }
    let unrelated = paths.logs_dir().join("notes.log");
    fs::write(&unrelated, "unrelated log").unwrap();
    let mut daemon = Daemon(start(home.path()).spawn().unwrap());
    wait_for_meta(&mut daemon, paths.daemon_socket()).await;
    daemon.terminate().await;

    let active = fs::read_link(paths.daemon_log()).unwrap();
    assert_eq!(active.parent(), Some(paths.logs_dir().as_path()));
    assert_eq!(mode(active), 0o600);
    let rotated = fs::read_dir(paths.logs_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("marsad.") && name.ends_with(".jsonl"))
        })
        .count();
    // The appender prunes by filesystem creation time when available.
    // These fixtures may share one timestamp, so their eviction order can vary.
    assert_eq!(rotated, 7);
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "unrelated log");
}

#[tokio::test]
async fn settings_updates_commit_before_ordered_events_and_leave_invalid_files_intact() {
    let home = TempDir::new_in("/tmp").unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    let mut daemon = Daemon(start(home.path()).spawn().unwrap());
    let channel = wait_for_channel(&mut daemon, paths.daemon_socket()).await;
    let mut settings = SettingsClient::new(channel.clone());
    let mut events = EventsClient::new(channel.clone())
        .subscribe(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();
    let first = events.message().await.unwrap().unwrap();
    let initial = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        first.body,
        Some(Body::Snapshot(snapshot)) if snapshot.settings_revision == initial.revision
    ));

    let update = UpdateSettingsRequest {
        request_id: "20000000-0000-4000-8000-000000000001".into(),
        settings_toml: "[vm]\ncpus = 4\n".into(),
        expected_revision: initial.revision,
    };
    let changed = settings.update(update.clone()).await.unwrap().into_inner();
    let identical = settings.update(update.clone()).await.unwrap().into_inner();
    assert_eq!(changed, identical);
    let written = fs::read_to_string(paths.config_file()).unwrap();
    assert_eq!(written, changed.settings_toml);
    assert_eq!(mode(paths.config_file()), 0o600);
    assert_eq!(changed.revision, initial.revision + 1);
    assert!(changed.restart_required);
    let stale = settings
        .update(UpdateSettingsRequest {
            request_id: "20000000-0000-4000-8000-000000000003".into(),
            ..update.clone()
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code(), tonic::Code::Aborted);
    assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), written);

    let invalid = settings
        .update(UpdateSettingsRequest {
            request_id: "20000000-0000-4000-8000-000000000002".into(),
            expected_revision: changed.revision,
            settings_toml: "[vm]\ncpus = 0\n".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
    assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), written);

    let event = events.message().await.unwrap().unwrap();
    assert_eq!(event.stream_id, first.stream_id);
    assert_eq!(event.sequence, first.sequence + 1);
    assert!(
        matches!(event.body, Some(Body::Settings(settings)) if settings.revision == changed.revision)
    );
    let mut engine = EngineClient::new(channel);
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped
    );
    let unavailable = engine
        .start(EngineActionRequest {
            request_id: update.request_id,
        })
        .await
        .unwrap_err();
    assert_eq!(unavailable.code(), tonic::Code::Unavailable);
    // A live subscription must close on daemon shutdown.
    daemon.terminate().await;
    assert!(events.message().await.unwrap().is_none());
}

fn start(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_marsad"));
    command
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

struct Daemon(Child);

impl Daemon {
    async fn terminate(&mut self) {
        kill(
            Pid::from_raw(i32::try_from(self.0.id()).unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "SIGTERM exit: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "daemon did not exit on SIGTERM");
            sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

async fn wait_for_meta(daemon: &mut Daemon, path: PathBuf) -> MetaClient<Channel> {
    MetaClient::new(wait_for_channel(daemon, path).await)
}

async fn wait_for_channel(daemon: &mut Daemon, path: PathBuf) -> Channel {
    let deadline = Instant::now() + WAIT;
    loop {
        let status: Option<ExitStatus> = daemon.0.try_wait().unwrap();
        assert!(
            status.is_none(),
            "daemon exited before serving Meta: {status:?}"
        );
        let target = path.clone();
        let endpoint =
            Endpoint::from_static("http://localhost").connect_timeout(Duration::from_millis(250));
        let connector = service_fn(move |_| {
            let path = target.clone();
            async move { UnixStream::connect(path).await.map(TokioIo::new) }
        });
        if let Ok(channel) = endpoint.connect_with_connector(connector).await {
            return channel;
        }
        assert!(
            Instant::now() < deadline,
            "daemon socket did not become reachable"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

fn mode(path: PathBuf) -> u32 {
    let metadata: io::Result<_> = fs::symlink_metadata(path);
    metadata.unwrap().permissions().mode() & 0o777
}
