//! Exercise the signed packaged daemon on Apple silicon through its Unix socket.
//! Run with MARSA_VZ_GUEST_DIR and MARSA_VZ_NETWORK_HELPER pointing to built artifacts.
//! Set MARSA_EXPECT_NATIVE_FALLBACK on a host where native NAT egress should fail.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::{
    env, fs,
    io::{Read, Write},
    os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

use hyper_util::rt::TokioIo;
use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::host::v1::{
    CapabilitiesRequest, CollectDiagnosticsRequest, DaemonEvent, DiskUsageRequest,
    EngineActionRequest, EngineHealthState, EngineState, EngineStatusRequest, GetSettingsRequest,
    NetworkMode, NetworkStatus, SubscribeRequest, UpdateSettingsRequest, VmStatsRequest,
    daemon_event::Body, diagnostics_client::DiagnosticsClient, disk_client::DiskClient,
    engine_client::EngineClient, events_client::EventsClient, meta_client::MetaClient,
    settings_client::SettingsClient, stats_client::StatsClient,
};
use nix::{
    errno::Errno,
    unistd::{Whence, lseek},
};
use tokio::{net::UnixStream, time::timeout};
use tonic::{
    Code, Streaming,
    transport::{Channel, Endpoint},
};
use tower::service_fn;

#[path = "packaged_runtime/recovery.rs"]
mod recovery;
#[path = "packaged_runtime/support.rs"]
mod support;
use support::{
    Daemon, action, active_network, assert_sparse_image_equal, checked, connect, disk_identity,
    memory, replace_settings, stage_bundle,
};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const WAIT: Duration = Duration::from_secs(20);
const BOOT: Duration = Duration::from_secs(120);

#[tokio::test]
#[ignore = "requires Apple silicon virtualization and built guest and network helper artifacts"]
async fn packaged_daemon_boots_and_preserves_its_private_data_disk() {
    assert!(!nix::unistd::geteuid().is_root());
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = Settings::default();
    config.vm.memory = Some(1024 * MIB);
    config.vm.idle_stop = false;
    let mut initial_settings = fs::File::create(paths.config_file()).unwrap();
    initial_settings
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    initial_settings
        .write_all(config.to_toml().unwrap().as_bytes())
        .unwrap();
    drop(initial_settings);

    let (daemon, guest) = stage_bundle(scratch.path());
    let logs = scratch.path().join("daemon.log");
    let mut process = Daemon(
        Command::new(daemon)
            .env("HOME", &home)
            .env("LC_ALL", "C")
            .stdout(fs::File::create(&logs).unwrap())
            .stderr(fs::File::options().append(true).open(&logs).unwrap())
            .spawn()
            .unwrap(),
    );
    let channel = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    let caps = MetaClient::new(channel.clone())
        .capabilities(CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(
        caps.backends
            .iter()
            .any(|backend| backend.backend == "vz" && backend.userspace_network),
        "{caps:?}"
    );
    let disk = paths.data_disk();
    assert!(!disk.exists());
    let mut storage = DiskClient::new(channel.clone());
    let absent_usage = storage
        .usage(DiskUsageRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(absent_usage.host_capacity_bytes, 0);
    assert!(!absent_usage.guest_usage_available);

    let mut events = EventsClient::new(channel.clone())
        .subscribe(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut engine = EngineClient::new(channel.clone());
    let started_at = Instant::now();
    let result = timeout(BOOT, engine.start(action(1))).await.unwrap();
    let console = || fs::read_to_string(paths.console_log()).unwrap_or_default();
    let ready = result
        .unwrap_or_else(|error| {
            panic!(
                "{error}\n{}\n{}",
                fs::read_to_string(&logs).unwrap(),
                console()
            )
        })
        .into_inner();
    assert_eq!(
        ready.state(),
        EngineState::Ready,
        "{ready:?}\n{}",
        console()
    );
    assert_eq!(ready.guest_health(), EngineHealthState::Ready);
    let bundle = DiagnosticsClient::new(channel.clone())
        .collect(CollectDiagnosticsRequest {
            request_id: "80000000-0000-4000-8000-000000000001".into(),
            include_agent_audit: false,
        })
        .await
        .unwrap()
        .into_inner();
    let file = fs::File::open(bundle.bundle_path).unwrap();
    let mut archive = tar::Archive::new(file);
    let mut engine_log = String::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == Path::new("engine-log.json") {
            entry.read_to_string(&mut engine_log).unwrap();
        }
    }
    assert!(engine_log.contains("docker_initialized"), "{engine_log}");

    let mut automatic_network = active_network(&mut events).await;
    assert_eq!(automatic_network.configured_mode(), NetworkMode::Auto);
    assert!(matches!(
        automatic_network.active_mode(),
        NetworkMode::Nat | NetworkMode::Userspace
    ));
    eprintln!("Initial automatic network: {automatic_network:?}");
    let initial_memory = memory(&channel).await;
    assert!(initial_memory > 800 * MIB);
    let identity = disk_identity(&disk);
    let info = fs::metadata(&disk).unwrap();
    assert_eq!(info.len(), 64 * GIB);
    eprintln!(
        "Fresh 64 GiB sparse disk reached Ready in {:?}.",
        started_at.elapsed()
    );
    assert_eq!(info.mode() & 0o7777, 0o600);
    assert_eq!(info.uid(), nix::unistd::geteuid().as_raw());
    assert!(info.blocks() * 512 < info.len());
    let usage = storage
        .usage(DiskUsageRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(usage.host_capacity_bytes, 64 * GIB);
    assert!(usage.host_allocated_bytes > 0);
    assert!(usage.host_allocated_bytes < usage.host_capacity_bytes);
    assert!(usage.guest_usage_available);
    assert!(usage.guest_capacity_bytes >= usage.guest_used_bytes);
    assert!(has_backup_exclusion(&disk));

    let expect_fallback = env::var_os("MARSA_EXPECT_NATIVE_FALLBACK").is_some();
    if expect_fallback
        && automatic_network
            .reason
            .as_deref()
            .is_some_and(|reason| reason.ends_with("_alternate_tcp"))
    {
        // A firewall prompt may outlast the first activation. Retry the same signed helper.
        let stopped = timeout(WAIT, engine.stop(action(9)))
            .await
            .unwrap()
            .unwrap()
            .into_inner();
        assert_eq!(stopped.state(), EngineState::Stopped);
        let ready = timeout(BOOT, engine.start(action(10)))
            .await
            .unwrap()
            .unwrap()
            .into_inner();
        assert_eq!(ready.state(), EngineState::Ready);
        automatic_network = active_network(&mut events).await;
        assert_eq!(automatic_network.configured_mode(), NetworkMode::Auto);
        assert_eq!(disk_identity(&disk), identity);
        eprintln!("Retried automatic network: {automatic_network:?}");
    }

    config.vm.memory = Some(1536 * MIB);
    config.disk.exclude_from_backup = false;
    config.network.mode = marsa_core::settings::NetworkMode::Userspace;
    let changed = replace_settings(&channel, &config, 2).await;
    assert!(changed);
    assert_eq!(memory(&channel).await, initial_memory);
    let ready = timeout(BOOT, engine.restart(action(3)))
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("{error}\n{}", console()))
        .into_inner();
    assert_eq!(ready.state(), EngineState::Ready);
    assert_eq!(ready.guest_health(), EngineHealthState::Ready);
    let new_network = active_network(&mut events).await;
    assert_eq!(new_network.configured_mode(), NetworkMode::Userspace);
    assert_eq!(new_network.active_mode(), NetworkMode::Userspace);
    assert!(
        !SettingsClient::new(channel.clone())
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner()
            .restart_required
    );
    assert!(!has_backup_exclusion(&disk));
    let updated_memory = memory(&channel).await;
    assert!(updated_memory >= initial_memory + 400 * MIB);
    assert_eq!(disk_identity(&disk), identity);
    eprintln!("Guest memory rose from {initial_memory} to {updated_memory} bytes on restart.");

    let stopped = timeout(WAIT, engine.stop(action(4)))
        .await
        .unwrap()
        .unwrap()
        .into_inner();
    assert_eq!(stopped.state(), EngineState::Stopped);
    assert_eq!(disk_identity(&disk), identity);

    let usage = storage
        .usage(DiskUsageRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(usage.host_capacity_bytes, 64 * GIB);
    assert_eq!(
        usage.host_allocated_bytes,
        fs::metadata(&disk).unwrap().blocks() * 512
    );
    assert!(!usage.guest_usage_available);

    // A faulty bundle must not resize the existing disk even with a larger setting.
    config.disk.data_size_limit = 65 * GIB;
    replace_settings(&channel, &config, 5).await;
    let before = scratch.path().join("before.img");
    checked(Command::new("/bin/cp").arg("-c").arg(&disk).arg(&before));
    let old_console = fs::read(paths.console_log()).unwrap();
    let rootfs = guest.join("rootfs.img");
    let absent = guest.join("rootfs.saved");
    let mut corrupt = None;
    for (step, diagnostic) in [
        (6, "No such file"),
        (7, "guest checksum mismatch"),
        (8, "invalid code signature or resource seal"),
    ] {
        match step {
            6 => fs::rename(&rootfs, &absent).unwrap(),
            7 => {
                fs::rename(&absent, &rootfs).unwrap();
                // Keep the filesystem header intact to exercise digest validation.
                let file = fs::File::options()
                    .read(true)
                    .write(true)
                    .open(&rootfs)
                    .unwrap();
                let mut byte = [0];
                file.read_exact_at(&mut byte, 4096).unwrap();
                file.write_all_at(&[byte[0] ^ 0xff], 4096).unwrap();
                corrupt = Some((file, byte));
            }
            8 => {
                let (file, byte) = corrupt.take().unwrap();
                file.write_all_at(&byte, 4096).unwrap();
                // The manifest and its digests are valid again, but the signed resource seal is broken.
                let manifest = guest.join("guest-manifest.json");
                fs::File::options()
                    .append(true)
                    .open(manifest)
                    .unwrap()
                    .write_all(b"\n")
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let outcome = timeout(WAIT, engine.start(action(step))).await.unwrap();
        assert_eq!(outcome.unwrap_err().code(), Code::Unavailable);
        let status = engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner();
        assert_eq!(status.state(), EngineState::Stopped, "{status:?}");
        assert_eq!(status.boot_stage.as_deref(), Some("prepare"));
        assert!(
            status
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains(diagnostic),
            "{status:?}"
        );
        assert_eq!(fs::read(paths.console_log()).unwrap(), old_console);
        assert_eq!(disk_identity(&disk), identity);
        assert_eq!(fs::metadata(&disk).unwrap().mode() & 0o7777, 0o600);
        assert_sparse_image_equal(&before, &disk);
    }

    drop(events);
    drop(engine);
    drop(channel);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(process.0.id().try_into().unwrap()),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    timeout(WAIT, async {
        loop {
            if let Some(status) = process.0.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{status}\n{}",
                    fs::read_to_string(&logs).unwrap()
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    if expect_fallback {
        assert_eq!(
            automatic_network.active_mode(),
            NetworkMode::Userspace,
            "{automatic_network:?}"
        );
        assert!(
            automatic_network
                .reason
                .as_deref()
                .is_some_and(|reason| reason.ends_with("_userspace_verified")),
            "{automatic_network:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, Docker CLI and Buildx"]
async fn packaged_docker_socket_preserves_cli_streams_and_bounded_memory() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(1536 * MIB);
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    let config = fs::File::create(paths.config_file()).unwrap();
    config
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    (&config)
        .write_all(settings.to_toml().unwrap().as_bytes())
        .unwrap();

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
    let mut engine = EngineClient::new(channel);
    let result = timeout(BOOT, engine.start(action(1))).await.unwrap();
    assert_eq!(
        result
            .unwrap_or_else(|error| panic!(
                "{error}\n{}\n{}",
                fs::read_to_string(&logs).unwrap(),
                fs::read_to_string(paths.console_log()).unwrap_or_default()
            ))
            .into_inner()
            .state(),
        EngineState::Ready,
    );

    let source = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"));
    let output = tokio::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/packaged_runtime/docker_proxy.py"))
        .arg(scratch.path())
        .arg(paths.docker_socket())
        .arg(source.join("image-rootfs.tar"))
        .arg(process.0.id().to_string())
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(paths.console_log()).unwrap_or_default(),
    );

    let stopped = timeout(WAIT, engine.stop(action(2)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
}

fn has_backup_exclusion(path: &Path) -> bool {
    // tmutil also reports exclusions inherited from the enclosing temporary directory.
    let output = Command::new("/usr/bin/xattr").arg(path).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .any(|attribute| attribute == "com.apple.metadata:com_apple_backup_excludeItem")
}
#[path = "packaged_runtime/on_demand.rs"]
mod on_demand;
