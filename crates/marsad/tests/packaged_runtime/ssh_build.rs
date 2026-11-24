//! Exercise the unmodified Buildx SSH provider through an entitled Marsa process.

use super::*;

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, Docker CLI and Buildx"]
async fn packaged_buildkit_ssh_session_clones_a_private_repository() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(1536 * MIB);
    settings.disk.data_size_limit = 4 * GIB;
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
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("SSH_AGENT_PID")
            .env("HOME", home)
            .env("LC_ALL", "C")
            .stdout(fs::File::create(&logs).unwrap())
            .stderr(fs::File::options().append(true).open(&logs).unwrap())
            .spawn()
            .unwrap(),
    );
    let channel = connect(&mut process.0, &paths.daemon_socket(), &logs).await;
    let mut engine = EngineClient::new(channel);
    let ready = timeout(BOOT, engine.start(action(1)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.into_inner().state(), EngineState::Ready);

    let output = tokio::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/packaged_runtime/ssh_build.py"))
        .arg(scratch.path())
        .arg(paths.docker_socket())
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("SSH_AGENT_PID")
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
