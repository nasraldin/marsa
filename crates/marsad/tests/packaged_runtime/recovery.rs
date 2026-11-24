//! Grow the real ext4 data disk after an abrupt host daemon exit.

use std::os::unix::process::ExitStatusExt;

use super::*;

const IMAGE: &str = "marsa-disk-recovery:local";
const BUSYBOX: &str = "/bin/busybox";
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
    let helper_pid = userspace_helper(process.0.id());
    let before = usage(&channel).await;
    assert_eq!(before.host_capacity_bytes, GIB);
    let identity = disk_identity(&paths.data_disk());
    let socket = paths.docker_socket();
    let fixture = offline_image(scratch.path());
    docker(
        &socket,
        &[
            "import",
            "--platform",
            "linux/arm64",
            fixture.to_str().unwrap(),
            IMAGE,
        ],
    )
    .await;
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
            BUSYBOX,
            "sh",
            "-c",
            "printf '%s' \"$1\" > /proof/marker && /bin/busybox sync",
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
            BUSYBOX,
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
    assert_helper_exits(helper_pid).await;

    let logs = scratch.path().join("second.log");
    let mut process = spawn(&daemon, &home, &logs);
    let recovered = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    start(&recovered, &paths, 4).await;
    let replacement_helper = userspace_helper(process.0.id());
    assert_ne!(replacement_helper, helper_pid);
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
            BUSYBOX,
            "cat",
            "/proof/marker",
        ],
    )
    .await;
    assert_eq!(marker, MARKER);
    drop(recovered);
    signal_running_daemon(&mut process).await;
    assert_clean_filesystem(&paths.data_disk());
    eprintln!(
        "Volume survived SIGKILL and guest capacity grew from {} to {} bytes.",
        before.guest_capacity_bytes, after.guest_capacity_bytes
    );
}

pub(super) fn userspace_helper(daemon_pid: u32) -> u32 {
    let processes = Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=,ppid=,comm="])
        .output()
        .unwrap();
    assert!(processes.status.success());
    let mut children = String::from_utf8_lossy(&processes.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let parent = fields.next()?.parse::<u32>().ok()?;
            (parent == daemon_pid && fields.last()?.ends_with("/marsa-net-helper")).then_some(pid)
        })
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 1, "expected a single private helper");
    children.pop().unwrap()
}

async fn assert_helper_exits(pid: u32) {
    let pid = pid.to_string();
    timeout(WAIT, async {
        loop {
            let helper = tokio::process::Command::new("/bin/ps")
                .args(["-p", &pid, "-o", "comm="])
                .output()
                .await
                .unwrap();
            if !String::from_utf8_lossy(&helper.stdout)
                .trim()
                .ends_with("/marsa-net-helper")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a private helper survived its daemon's SIGKILL");
}

pub(super) async fn signal_running_daemon(process: &mut Daemon) {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(process.0.id().try_into().unwrap()),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let status = timeout(WAIT, async {
        loop {
            if let Some(status) = process.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success(), "{status}");
}

fn assert_clean_filesystem(path: &Path) {
    // Verify the primary ext4 superblock after the daemon has released the disk.
    // https://www.kernel.org/doc/html/v7.0/filesystems/ext4/super.html
    const EXT4_SUPERBLOCK: u64 = 1024;
    const NEEDS_RECOVERY: u32 = 0x4;
    let mut superblock = [0_u8; 100];
    fs::File::open(path)
        .unwrap()
        .read_exact_at(&mut superblock, EXT4_SUPERBLOCK)
        .unwrap();
    assert_eq!(&superblock[0x38..0x3a], &[0x53, 0xef]);
    let state = u16::from_le_bytes(superblock[0x3a..0x3c].try_into().unwrap());
    let incompatible = u32::from_le_bytes(superblock[0x60..0x64].try_into().unwrap());
    assert_eq!(state, 0x1, "ext4 was not cleanly unmounted");
    assert_eq!(
        incompatible & NEEDS_RECOVERY,
        0,
        "ext4 needs journal replay"
    );
}

pub(super) fn offline_image(scratch: &Path) -> PathBuf {
    // Use the locally built, pinned guest tools so a registry outage cannot
    // turn the disk recovery test into a networking test.
    let guest = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").unwrap());
    let contents = scratch.join("recovery-image");
    fs::create_dir(&contents).unwrap();
    let members = ["./bin/busybox", "./lib/ld-musl-aarch64.so.1"];
    checked(
        Command::new("/usr/bin/tar")
            .arg("-xf")
            .arg(guest.join("rootfs.tar"))
            .arg("-C")
            .arg(&contents)
            .args(members),
    );
    let archive = scratch.join("recovery-image.tar");
    checked(
        Command::new("/usr/bin/tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(contents)
            .args(members),
    );
    archive
}

pub(super) fn spawn(daemon: &Path, home: &Path, log: &Path) -> Daemon {
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

pub(super) async fn start(channel: &Channel, paths: &MarsaPaths, step: u32) {
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

pub(super) async fn docker(socket: &Path, args: &[&str]) -> String {
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

// Run this separately on the Tier 2 host. The data image, container, and
// archive all belong to the fixture and are removed with its temporary home.
#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, and the Docker CLI"]
async fn packaged_daemon_reclaims_disposable_data_from_the_sparse_image() {
    use marsa_proto::host::v1::DiskReclaimRequest;

    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let logs = scratch.path().join("reclaim.log");
    let mut process = spawn(&daemon, &home, &logs);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    let mut disk = DiskClient::new(channel.clone());
    let stopped = DiskReclaimRequest {
        request_id: action(1).request_id,
    };
    assert_eq!(
        disk.reclaim(stopped).await.unwrap_err().code(),
        Code::FailedPrecondition
    );
    assert!(!paths.data_disk().exists(), "reclaim booted a stopped VM");

    let mut config = Settings::default();
    config.disk.data_size_limit = 8 * GIB;
    config.vm.memory = Some(1024 * MIB);
    config.vm.idle_stop = false;
    config.network.mode = marsa_core::settings::NetworkMode::Userspace;
    replace_settings(&channel, &config, 2).await;
    start(&channel, &paths, 3).await;
    let baseline = usage(&channel).await;
    let identity = disk_identity(&paths.data_disk());

    const SCRATCH_CONTAINER: &str = "marsa-reclaim-fixture";
    let socket = paths.docker_socket();
    let fixture = offline_image(scratch.path());
    docker(
        &socket,
        &[
            "import",
            "--platform",
            "linux/arm64",
            fixture.to_str().unwrap(),
            IMAGE,
        ],
    )
    .await;
    docker(
        &socket,
        &[
            "run",
            "--name",
            SCRATCH_CONTAINER,
            "--pull=never",
            "--network=none",
            IMAGE,
            BUSYBOX,
            "sh",
            "-c",
            "/bin/busybox dd if=/dev/urandom of=/payload bs=1M count=3072 && /bin/busybox sync",
        ],
    )
    .await;
    let filled = wait_disk_usage(&channel, |sample| {
        sample.guest_used_bytes > baseline.guest_used_bytes + 2 * GIB
    })
    .await;
    docker(&socket, &["rm", SCRATCH_CONTAINER]).await;
    let before = wait_disk_usage(&channel, |sample| {
        filled.guest_used_bytes > sample.guest_used_bytes + 2 * GIB
    })
    .await;
    let freed = filled.guest_used_bytes - before.guest_used_bytes;
    let trim = DiskReclaimRequest {
        request_id: action(4).request_id,
    };
    let reclaimed = disk.reclaim(trim.clone()).await.unwrap().into_inner();
    assert_eq!(reclaimed, disk.reclaim(trim).await.unwrap().into_inner());
    let after = reclaimed.usage.unwrap();
    assert_eq!(disk_identity(&paths.data_disk()), identity);
    assert_eq!(after.host_capacity_bytes, 8 * GIB);
    assert!(
        reclaimed.host_reclaimed_bytes >= freed * 9 / 10,
        "discarded {} bytes, guest freed {freed} bytes, host reclaimed {} bytes",
        reclaimed.guest_trimmed_bytes,
        reclaimed.host_reclaimed_bytes,
    );
    assert!(
        before
            .host_allocated_bytes
            .saturating_sub(after.host_allocated_bytes)
            >= freed * 9 / 10,
        "host sparse allocation did not fall by 90%"
    );
    eprintln!(
        "Guest freed {freed} bytes, explicit trim released {} host bytes.",
        reclaimed.host_reclaimed_bytes
    );
    if let Some(cli) = env::var_os("MARSA_TEST_MARSA") {
        let output = timeout(
            BOOT,
            tokio::process::Command::new(cli)
                .env("HOME", &home)
                .args(["disk", "reclaim", "--json"])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["kind"], "disk_reclaim");
        assert_eq!(json["usage"]["host"]["capacity_bytes"], 8 * GIB);
        assert!(json["host_reclaimed_bytes"].is_u64());
        assert!(json["guest_trimmed_bytes"].is_u64());
        eprintln!("Signed VM CLI reclaim response {json}");
    }
    drop(disk);
    drop(channel);
    signal_running_daemon(&mut process).await;
    assert_clean_filesystem(&paths.data_disk());
}

async fn wait_disk_usage(
    channel: &Channel,
    condition: impl Fn(&marsa_proto::host::v1::DiskUsageResponse) -> bool,
) -> marsa_proto::host::v1::DiskUsageResponse {
    timeout(WAIT, async {
        loop {
            let sample = usage(channel).await;
            if condition(&sample) {
                return sample;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, and the Docker CLI"]
async fn packaged_cleanup_preview_lists_disposable_storage_without_removal() {
    use marsa_proto::host::v1::{DiskCategory, PrunePreviewRequest};

    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let logs = scratch.path().join("preview.log");
    let mut process = spawn(&daemon, &home, &logs);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    let mut config = Settings::default();
    config.disk.data_size_limit = GIB;
    config.vm.memory = Some(1024 * MIB);
    config.vm.idle_stop = false;
    config.network.mode = marsa_core::settings::NetworkMode::Userspace;
    replace_settings(&channel, &config, 121).await;
    start(&channel, &paths, 122).await;

    let socket = paths.docker_socket();
    let fixture = offline_image(scratch.path());
    let fixture = fixture.to_str().unwrap();
    let version = docker(&socket, &["version", "--format", "{{.Server.Version}}"]).await;
    assert_eq!(version.trim(), "29.10.0");
    docker(
        &socket,
        &["import", "--platform", "linux/arm64", fixture, IMAGE],
    )
    .await;
    let unused = docker(
        &socket,
        &[
            "import",
            "--platform",
            "linux/arm64",
            "--change",
            "LABEL preview=unused",
            fixture,
            "marsa-preview:unused",
        ],
    )
    .await;
    let container = docker(
        &socket,
        &[
            "create",
            "--name",
            "marsa-preview-stopped",
            "--pull=never",
            "--network=none",
            IMAGE,
            BUSYBOX,
            "true",
        ],
    )
    .await;
    let volume = "marsa-preview-volume";
    docker(&socket, &["volume", "create", volume]).await;

    let mut disk = DiskClient::new(channel.clone());
    let default = disk
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(
        default.candidates.iter().any(|row| {
            row.category() == DiskCategory::Containers && row.id == container.trim()
        }),
        "{default:?}"
    );
    assert!(
        default
            .candidates
            .iter()
            .any(|row| { row.category() == DiskCategory::Images && row.id == unused.trim() }),
        "{default:?}"
    );
    assert!(
        default
            .candidates
            .iter()
            .all(|row| row.category() != DiskCategory::Volumes)
    );

    let selected = disk
        .prune_preview(PrunePreviewRequest {
            categories: vec![DiskCategory::Volumes as i32],
            volume_names: vec![volume.into()],
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(selected.candidates.len(), 1, "{selected:?}");
    assert_eq!(selected.candidates[0].id, volume);
    assert_eq!(
        docker(
            &socket,
            &["volume", "inspect", "--format", "{{.Name}}", volume]
        )
        .await
        .trim(),
        volume
    );
    assert_eq!(
        docker(
            &socket,
            &["inspect", "--format", "{{.Id}}", container.trim()]
        )
        .await
        .trim(),
        container.trim()
    );
    assert_eq!(
        docker(
            &socket,
            &["image", "inspect", "--format", "{{.Id}}", unused.trim()]
        )
        .await
        .trim(),
        unused.trim()
    );

    eprintln!("Docker {version} previewed the exact fixture IDs and preserved their resources.");
    drop(disk);
    drop(channel);
    signal_running_daemon(&mut process).await;
    assert_clean_filesystem(&paths.data_disk());
}
