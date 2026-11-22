//! Qualify scoped resolver changes through the intact packaged user-space path.

use super::*;

use marsa_proto::host::v1::{NetworkStatusRequest, network_client::NetworkClient};

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts and the pinned Docker CLI"]
async fn scoped_dns_updates_reach_existing_containers_without_restart() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();

    let mut settings = Settings::default();
    settings.vm.memory = Some(1536 * MIB);
    settings.vm.idle_stop = false;
    settings.disk.data_size_limit = GIB;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    let mut config = fs::File::options()
        .write(true)
        .create_new(true)
        .open(paths.config_file())
        .unwrap();
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
    let mut network = NetworkClient::new(channel);
    let stopped_network = network
        .status(NetworkStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stopped_network.configured_mode(), NetworkMode::Userspace);
    assert_eq!(stopped_network.active_mode(), NetworkMode::Unspecified);

    let started = timeout(BOOT, engine.start(action(1)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.into_inner().state(), EngineState::Ready);
    let running_network = timeout(WAIT, async {
        loop {
            let status = network
                .status(NetworkStatusRequest {})
                .await
                .unwrap()
                .into_inner();
            if status.guest_address.is_some() && status.gateway_address.is_some() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(running_network.configured_mode(), NetworkMode::Userspace);
    assert_eq!(running_network.active_mode(), NetworkMode::Userspace);
    assert_eq!(
        running_network.guest_address.as_deref(),
        Some("192.168.127.2")
    );
    assert_eq!(
        running_network.gateway_address.as_deref(),
        Some("192.168.127.1")
    );

    let guest = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"));
    let output = tokio::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/packaged_runtime/fallback_dns.py"))
        .arg(scratch.path())
        .arg(paths.docker_socket())
        .arg(guest.join("image-rootfs.tar"))
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
    assert_eq!(
        network
            .status(NetworkStatusRequest {})
            .await
            .unwrap()
            .into_inner(),
        stopped_network
    );
}
