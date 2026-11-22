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
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::{sleep, timeout},
};
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
    assert_eq!(mode(paths.docker_socket()), 0o600);
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
    assert!(!paths.docker_socket().exists());
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
async fn docker_unavailability_includes_a_localized_error() {
    for (locale, expected) in [
        ("C", "Marsa could not connect"),
        ("ar", "تعذر على مارسا الاتصال"),
    ] {
        let home = TempDir::new_in("/tmp").unwrap();
        let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
        let mut daemon = Daemon(start(home.path()).env("LC_ALL", locale).spawn().unwrap());
        wait_for_meta(&mut daemon, paths.daemon_socket()).await;
        let mut docker = UnixStream::connect(paths.docker_socket()).await.unwrap();
        docker
            .write_all(b"GET /_ping HTTP/1.1\r\nHost: docker\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        timeout(WAIT, docker.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 503 "));
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert!(body["message"].as_str().unwrap().starts_with(expected));
        daemon.terminate().await;
    }
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
    assert!(paths.docker_socket().exists());
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
    for docker in [false, true] {
        let home = TempDir::new_in("/tmp").unwrap();
        let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
        fs::create_dir_all(paths.run_dir()).unwrap();
        let (socket, other_socket) = if docker {
            (paths.docker_socket(), paths.daemon_socket())
        } else {
            (paths.daemon_socket(), paths.docker_socket())
        };
        let sentinel = home.path().join("sentinel");
        fs::write(&sentinel, "keep this file").unwrap();
        symlink(&sentinel, &socket).unwrap();
        let rejected = start(home.path()).output().unwrap();
        assert!(!rejected.status.success());
        assert!(
            String::from_utf8(rejected.stderr)
                .unwrap()
                .contains("unexpected owner or file type")
        );
        assert_eq!(fs::read_link(socket).unwrap(), sentinel);
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep this file");
        assert!(!other_socket.exists());
        assert!(!paths.daemon_pid_file().exists());
    }
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

#[tokio::test]
async fn share_grants_protect_the_configured_home_and_the_effective_account_home() {
    use marsa_proto::host::v1::{AddShareRequest, shares_client::SharesClient};
    use nix::unistd::{User, geteuid};

    let home = TempDir::new_in("/tmp").unwrap();
    let account = User::from_uid(geteuid()).unwrap().unwrap();
    assert_ne!(home.path(), account.dir);
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    let mut daemon = Daemon(start(home.path()).spawn().unwrap());
    let channel = wait_for_channel(&mut daemon, paths.daemon_socket()).await;
    let mut shares = SharesClient::new(channel);
    for (index, home) in [home.path(), account.dir.as_path()].into_iter().enumerate() {
        let result = shares
            .add(AddShareRequest {
                request_id: format!("50000000-0000-4000-8000-{index:012}"),
                host_path: home.join(".ssh").to_str().unwrap().into(),
                persist: true,
                ..Default::default()
            })
            .await;
        assert!(matches!(result, Err(ref error) if error.code() == tonic::Code::PermissionDenied));
    }
    daemon.terminate().await;
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test]
#[ignore = "requires the verified M4-03 Docker CLI at MARSA_TEST_DOCKER_CLI"]
async fn pinned_cli_sees_the_context_and_retains_user_selection() {
    use marsa_proto::host::v1::{
        EnsureDockerContextRequest, docker_discovery_client::DockerDiscoveryClient,
    };

    let cli = PathBuf::from(std::env::var_os("MARSA_TEST_DOCKER_CLI").unwrap())
        .canonicalize()
        .unwrap();
    let version = Command::new(&cli)
        .arg("--version")
        .env_clear()
        .output()
        .unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .contains("Docker version 29.10.0")
    );
    let home = tempfile::Builder::new()
        .prefix("m4=, \"")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = marsa_core::paths::MarsaPaths::from_home(home.path()).unwrap();
    let config = home.path().join("docker-fixture");
    fs::create_dir(&config).unwrap();
    let app = home.path().join("Marsa.app");
    let binary = app.join("Contents/MacOS/marsad");
    let packaged_cli = app.join("Contents/Resources/bin/docker");
    for path in [&binary, &packaged_cli] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    fs::copy(env!("CARGO_BIN_EXE_marsad"), &binary).unwrap();
    fs::copy(&cli, &packaged_cli).unwrap();
    fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.marsa.context-test</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleExecutable</key><string>marsad</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#,
    )
    .unwrap();
    let sign = |path: &Path| {
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(path)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    sign(&packaged_cli);
    sign(&app);
    let mut daemon = Daemon(
        Command::new("/bin/sh")
            .args(["-c", "umask 0777; exec \"$@\"", "marsa-context-fixture"])
            .arg(binary)
            .env_clear()
            .env("HOME", home.path())
            .env("DOCKER_CONFIG", &config)
            .env("DOCKER_CONTEXT", "unrelated-fixture")
            .env("DOCKER_HOST", "unix:///tmp/unrelated-fixture.sock")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let channel = wait_for_channel(&mut daemon, paths.daemon_socket()).await;
    let run = |args: &[&str]| {
        let output = Command::new(&cli)
            .env_clear()
            .env("HOME", home.path())
            .arg("--config")
            .arg(&config)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Docker command failed");
        String::from_utf8(output.stdout).unwrap()
    };
    run(&[
        "context",
        "create",
        "unrelated",
        "--docker",
        "host=unix:///tmp/unrelated.sock",
    ]);
    let unrelated = run(&["context", "inspect", "unrelated"]);
    let config_file = config.join("config.json");
    let original = b"{ \"currentContext\" : \"unrelated\", \"credsStore\" : \"fixture\" }\n";
    fs::write(&config_file, original).unwrap();
    assert_eq!(run(&["context", "show"]).trim(), "unrelated");
    assert!(
        !run(&["context", "ls", "--format", "{{.Name}}"])
            .lines()
            .any(|name| name == "marsa")
    );

    let mut discovery = DockerDiscoveryClient::new(channel);
    let request = |id| EnsureDockerContextRequest {
        request_id: format!("34000000-0000-4000-8000-{id:012}"),
    };
    let mut concurrent = discovery.clone();
    let (a, b) = tokio::join!(
        discovery.ensure_context(request(1)),
        concurrent.ensure_context(request(2)),
    );
    let a = a.unwrap().into_inner();
    let b = b.unwrap().into_inner();
    assert_ne!(a.created, b.created);
    let endpoint = a.endpoint.as_ref().unwrap();
    assert_eq!(Some(endpoint), b.endpoint.as_ref());
    assert_eq!(
        discovery
            .ensure_context(request(1))
            .await
            .unwrap()
            .into_inner(),
        a
    );
    let inspected = run(&[
        "context",
        "inspect",
        "marsa",
        "--format",
        "{{.Endpoints.docker.Host}}",
    ]);
    assert_eq!(inspected.trim(), endpoint.docker_host);
    assert_eq!(
        endpoint.socket_path,
        paths.docker_socket().to_str().unwrap()
    );
    let context_path = run(&[
        "context",
        "inspect",
        "marsa",
        "--format",
        "{{.Storage.MetadataPath}}",
    ]);
    let context_path = Path::new(context_path.trim());
    assert_eq!(mode(context_path.to_owned()), 0o700);
    assert_eq!(mode(context_path.join("meta.json")), 0o600);
    assert_eq!(run(&["context", "show"]).trim(), "unrelated");
    assert_eq!(
        run(&["--context", "marsa", "context", "show"]).trim(),
        "marsa"
    );
    assert_eq!(run(&["context", "inspect", "unrelated"]), unrelated);
    assert_eq!(fs::read(config_file).unwrap(), original);
    assert!(!home.path().join(".docker").exists());
    assert!(!paths.data_disk().exists());
    daemon.terminate().await;
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
