//! Grow the real ext4 data disk after an abrupt host daemon exit.

use std::os::unix::process::ExitStatusExt;

use super::*;

const IMAGE: &str = "docker.io/library/alpine:3.24.4@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf";
const VOLUME: &str = "marsa-disk-recovery:/proof";
const MARKER: &str = "marsa-volume-survived";

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, and the Docker CLI"]
async fn packaged_daemon_preserves_volume_and_grows_disk_after_sigkill() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let mut process = spawn(&daemon, &home, &scratch.path().join("first.log"));
    let channel = connect(
        &mut process.0,
        &paths.daemon_socket(),
        &scratch.path().join("first.log"),
    )
    .await;
    let mut config = Settings::default();
    config.disk.data_size_limit = GIB;
    config.vm.memory = Some(1024 * MIB);
    config.vm.idle_stop = false;
    config.network.mode = marsa_core::settings::NetworkMode::Userspace;
    replace_settings(&channel, &config, 1).await;
    start(&channel, &paths, 2).await;
    let before = usage(&channel).await;
    assert_eq!(before.host_capacity_bytes, GIB);
    let identity = disk_identity(&paths.data_disk());
    let socket = paths.docker_socket();
    docker(&socket, &["pull", "--platform", "linux/arm64", IMAGE]).await;
    docker(
        &socket,
        &[
            "run",
            "--rm",
            "--pull=never",
            "--network=none",
            "-v",
            VOLUME,
            IMAGE,
            "sh",
            "-c",
            "printf '%s' \"$1\" > /proof/marker && sync",
            "sh",
            MARKER,
        ],
    )
    .await;
    docker(
        &socket,
        &[
            "run",
            "--detach",
            "--pull=never",
            "--network=none",
            "-v",
            VOLUME,
            IMAGE,
            "sleep",
            "600",
        ],
    )
    .await;
    assert_eq!(docker(&socket, &["ps", "-q"]).await.lines().count(), 1);
    config.disk.data_size_limit = 2 * GIB;
    assert!(replace_settings(&channel, &config, 3).await);
    process.0.kill().unwrap();
    assert_eq!(process.0.wait().unwrap().signal(), Some(9));
    drop(channel);

    let logs = scratch.path().join("second.log");
    let mut process = spawn(&daemon, &home, &logs);
    let recovered = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    start(&recovered, &paths, 4).await;
    let after = usage(&recovered).await;
    assert_eq!(after.host_capacity_bytes, 2 * GIB);
    assert!(after.guest_capacity_bytes > before.guest_capacity_bytes + 768 * MIB);
    assert_eq!(disk_identity(&paths.data_disk()), identity);
    let marker = docker(
        &socket,
        &[
            "run",
            "--rm",
            "--pull=never",
            "--network=none",
            "-v",
            VOLUME,
            IMAGE,
            "cat",
            "/proof/marker",
        ],
    )
    .await;
    assert_eq!(marker, MARKER);
    eprintln!(
        "Volume survived SIGKILL and guest capacity grew from {} to {} bytes.",
        before.guest_capacity_bytes, after.guest_capacity_bytes
    );
}

fn spawn(daemon: &Path, home: &Path, log: &Path) -> Daemon {
    let output = fs::File::create(log).unwrap();
    Daemon(
        Command::new(daemon)
            .env("HOME", home)
            .env("LC_ALL", "C")
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap(),
    )
}

async fn start(channel: &Channel, paths: &MarsaPaths, step: u32) {
    let ready = timeout(BOOT, EngineClient::new(channel.clone()).start(action(step)))
        .await
        .unwrap()
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{}",
                fs::read_to_string(paths.console_log()).unwrap_or_default()
            )
        })
        .into_inner();
    assert_eq!(ready.state(), EngineState::Ready, "{ready:?}");
}

async fn usage(channel: &Channel) -> marsa_proto::host::v1::DiskUsageResponse {
    memory(channel).await;
    let usage = DiskClient::new(channel.clone())
        .usage(DiskUsageRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(usage.guest_usage_available, "{usage:?}");
    usage
}

async fn docker(socket: &Path, args: &[&str]) -> String {
    let binary = env::var_os("MARSA_TEST_DOCKER").unwrap_or_else(|| "docker".into());
    let config = tempfile::tempdir().unwrap();
    let output = timeout(
        BOOT,
        tokio::process::Command::new(binary)
            .env_remove("DOCKER_HOST")
            .env_remove("DOCKER_CONTEXT")
            .env_remove("DOCKER_TLS_VERIFY")
            .env_remove("DOCKER_API_VERSION")
            .arg("--config")
            .arg(config.path())
            .arg("--host")
            .arg(format!("unix://{}", socket.display()))
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap_or_else(|_| panic!("Docker command timed out: {args:?}"))
    .unwrap();
    assert!(
        output.status.success(),
        "{args:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
