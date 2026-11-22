//! Verify an upstream development server's live reload through the real Docker socket.

use super::*;

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, Docker Compose and verified Hugo archive"]
async fn packaged_compose_watch_reloads_hugo_after_host_saves() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(1536 * MIB);
    settings.disk.data_size_limit = GIB;
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    assert!(settings.shares.is_empty());

    let mut config = fs::File::options()
        .create_new(true)
        .write(true)
        .open(paths.config_file())
        .unwrap();
    config
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    config
        .write_all(settings.to_toml().unwrap().as_bytes())
        .unwrap();
    drop(config);

    let guest = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR"));
    let hugo = env::var_os("MARSA_VZ_WATCH_HUGO_ARCHIVE").expect("stage the Hugo fixture archive");
    let (daemon, _) = stage_bundle(root.path());
    let logs = root.path().join("daemon.log");
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
    let ready = timeout(BOOT, engine.start(action(1))).await.unwrap();
    assert_eq!(
        ready
            .unwrap_or_else(|error| panic!(
                "{error}\n{}\n{}",
                fs::read_to_string(&logs).unwrap(),
                fs::read_to_string(paths.console_log()).unwrap_or_default()
            ))
            .into_inner()
            .state(),
        EngineState::Ready,
    );

    let output = timeout(
        BOOT,
        tokio::process::Command::new("python3")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/packaged_runtime/compose_watch.py"),
            )
            .arg(root.path())
            .arg(paths.docker_socket())
            .arg(guest.join("image-rootfs.tar"))
            .arg(hugo)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
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
