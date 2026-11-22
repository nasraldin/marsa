//! Verify Docker CLI activation and workload-sensitive idle stop in the signed bundle.

use super::*;

const IDLE_SECONDS: u32 = 4;

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, and the Docker CLI"]
async fn packaged_docker_starts_once_on_demand_and_restarts_after_idle() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_timeout = IDLE_SECONDS;
    let mut config = fs::File::create(paths.config_file()).unwrap();
    config
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    config
        .write_all(settings.to_toml().unwrap().as_bytes())
        .unwrap();
    drop(config);

    let (daemon, _) = stage_bundle(scratch.path());
    let logs = scratch.path().join("daemon.log");
    let mut process = Daemon(
        Command::new(daemon)
            .env("HOME", home)
            .env("LC_ALL", "C")
            .stdout(fs::File::create(&logs).unwrap())
            .stderr(fs::File::options().append(true).open(&logs).unwrap())
            .spawn()
            .unwrap(),
    );
    let channel = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    let mut engine = EngineClient::new(channel.clone());
    let mut events = EventsClient::new(channel)
        .subscribe(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        events.message().await.unwrap().unwrap().body,
        Some(Body::Snapshot(snapshot))
            if snapshot.engine.as_ref().is_some_and(|engine| engine.state() == EngineState::Stopped)
    ));

    let docker = Docker {
        host: format!("unix://{}", paths.docker_socket().display()),
        config: scratch.path().join("docker"),
    };
    fs::create_dir(&docker.config).unwrap();
    let fixture = archive(scratch.path());
    let mut clients = tokio::task::JoinSet::new();
    client(&mut clients, &docker);
    let mut activations = 0;
    wait_for(&mut events, &mut activations, EngineState::Starting).await;
    for _ in 1..10 {
        client(&mut clients, &docker);
    }
    wait_for(&mut events, &mut activations, EngineState::Ready).await;
    while let Some(result) = clients.join_next().await {
        assert_eq!(result.unwrap().trim(), "");
    }
    assert_eq!(activations, 1);
    eprintln!("Ten Docker CLI calls joined one cold activation.");

    docker
        .run(&["import", fixture.to_str().unwrap(), "marsa-idle-fixture"])
        .await;
    docker
        .run(&[
            "run",
            "-d",
            "--name",
            "workload",
            "marsa-idle-fixture",
            "/bin/busybox",
            "sleep",
            "120",
        ])
        .await;
    let quiet = Duration::from_secs(u64::from(IDLE_SECONDS) + 3);
    tokio::time::sleep(quiet).await;
    assert_eq!(status(&mut engine).await, EngineState::Ready);
    docker.run(&["pause", "workload"]).await;
    tokio::time::sleep(quiet).await;
    assert_eq!(status(&mut engine).await, EngineState::Ready);
    docker.run(&["unpause", "workload"]).await;
    docker.run(&["stop", "--time", "1", "workload"]).await;
    docker
        .run(&[
            "run",
            "-d",
            "--restart=always",
            "--name",
            "crashing",
            "marsa-idle-fixture",
            "/bin/busybox",
            "false",
        ])
        .await;
    timeout(WAIT, async {
        loop {
            if docker
                .run(&["inspect", "--format", "{{.State.Status}}", "crashing"])
                .await
                .trim()
                == "restarting"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(quiet).await;
    assert_eq!(status(&mut engine).await, EngineState::Ready);
    docker.run(&["stop", "--time", "1", "crashing"]).await;
    // An always-restart container starts again on engine restart even after a manual stop.
    docker.run(&["rm", "crashing"]).await;
    wait_for(&mut events, &mut activations, EngineState::Idle).await;
    wait_for(&mut events, &mut activations, EngineState::Stopped).await;
    assert_eq!(activations, 1);
    eprintln!(
        "Running, paused, and restarting containers deferred idle stop. The VM stopped once the workload stopped."
    );

    assert_eq!(docker.run(&["ps", "-q"]).await.trim(), "");
    wait_for(&mut events, &mut activations, EngineState::Ready).await;
    assert_eq!(activations, 2);
    assert_eq!(status(&mut engine).await, EngineState::Ready);
    eprintln!("The next Docker CLI call started a fresh activation.");
}

fn client(clients: &mut tokio::task::JoinSet<String>, docker: &Docker) {
    let docker = docker.clone();
    clients.spawn(async move { docker.run(&["ps", "-q"]).await });
}

async fn wait_for(
    events: &mut Streaming<DaemonEvent>,
    activations: &mut usize,
    expected: EngineState,
) {
    timeout(BOOT, async {
        loop {
            if let Some(Body::Engine(engine)) = events.message().await.unwrap().unwrap().body {
                if engine.state() == EngineState::Starting {
                    *activations += 1;
                }
                if engine.state() == expected {
                    return;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the packaged VM did not enter {expected:?}"));
}

async fn status(engine: &mut EngineClient<Channel>) -> EngineState {
    engine
        .status(EngineStatusRequest {})
        .await
        .unwrap()
        .into_inner()
        .state()
}

fn archive(scratch: &Path) -> PathBuf {
    let root = scratch.join("rootfs");
    let guest_dir = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    fs::create_dir(&root).unwrap();
    checked(
        Command::new("/usr/bin/tar")
            .arg("-xf")
            .arg(PathBuf::from(guest_dir).join("image-rootfs.tar"))
            .arg("-C")
            .arg(&root)
            .args(["./bin/busybox", "./lib/ld-musl-aarch64.so.1"]),
    );
    let archive = scratch.join("workload.tar");
    checked(
        Command::new("/usr/bin/tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&root)
            .args(["bin", "lib"]),
    );
    archive
}

#[derive(Clone)]
struct Docker {
    host: String,
    config: PathBuf,
}

impl Docker {
    async fn run(&self, args: &[&str]) -> String {
        let mut command = tokio::process::Command::new("docker");
        command
            .arg("--config")
            .arg(&self.config)
            .args(["--host", &self.host])
            .args(args);
        for (key, _) in env::vars_os() {
            if key.to_string_lossy().starts_with("DOCKER_") {
                command.env_remove(key);
            }
        }
        let output = timeout(BOOT, command.kill_on_drop(true).output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
