//! Run the G4 clients on an isolated engine and a single explicitly shared directory.

use super::*;

#[tokio::test]
#[ignore = "requires the Tier 2 Docker bundle and conformance client toolchains"]
async fn real_clients_use_the_packaged_engine() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    let share = root.join("share");
    let config = root.join("docker-config");
    for dir in [&home, &share, &config] {
        fs::create_dir(dir).unwrap();
    }
    let docker = PathBuf::from(env::var_os("MARSA_TEST_DOCKER").expect("set MARSA_TEST_DOCKER"))
        .canonicalize()
        .unwrap();
    std::os::unix::fs::symlink(
        docker.parent().unwrap().join("cli-plugins"),
        config.join("cli-plugins"),
    )
    .unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(&root);
    let log = root.join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    let mut settings = Settings::default();
    settings.shares.push(share.clone());
    settings.vm.memory = Some(2 * GIB);
    settings.vm.idle_stop = false;
    settings.disk.data_size_limit = 8 * GIB;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    replace_settings(&channel, &settings, 1).await;
    recovery::start(&channel, &paths, 2).await;
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance");
    for (program, script) in [
        ("python3", "engine/check.py"),
        ("python3", "differences/check.py"),
        ("bash", "tools/run.sh"),
    ] {
        let mut cmd = tokio::process::Command::new(program);
        cmd.arg(suite.join(script));
        if program == "python3" {
            cmd.arg(paths.docker_socket());
        }
        let status = cmd
            .env(
                "DOCKER_HOST",
                format!("unix://{}", paths.docker_socket().display()),
            )
            .env_remove("DOCKER_CONTEXT")
            .env("DOCKER_CONFIG", &config)
            .env("MARSA_TEST_DOCKER", &docker)
            .env("MARSA_CONFORMANCE_SHARE", &share)
            .kill_on_drop(true)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "{script} exited with {status}");
    }
    EngineClient::new(channel).stop(action(3)).await.unwrap();
    recovery::signal_running_daemon(&mut process).await;
}
