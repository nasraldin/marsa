//! Verify the public CLI against a signed daemon and a private guest.

use super::{recovery, *};
use serde_json::{Value, json};

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts and the built Marsa CLI"]
async fn packaged_cli_saves_network_mode_for_the_next_start() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = stage_bundle(scratch.path());
    let log = scratch.path().join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    let mut settings = Settings::default();
    settings.disk.data_size_limit = GIB;
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    replace_settings(&channel, &settings, 1).await;
    assert_eq!(mode(&home, None).await, expected("userspace", None, false));

    recovery::start(&channel, &paths, 2).await;
    let helper = recovery::userspace_helper(process.0.id());
    assert_eq!(
        mode(&home, None).await,
        expected("userspace", Some("userspace"), false)
    );
    assert_eq!(
        mode(&home, Some("auto")).await,
        expected("auto", Some("userspace"), true)
    );
    assert_eq!(
        mode(&home, None).await,
        expected("auto", Some("userspace"), true)
    );
    assert_eq!(recovery::userspace_helper(process.0.id()), helper);

    let mut engine = EngineClient::new(channel);
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Ready
    );
    let configured = fs::read_to_string(paths.config_file()).unwrap();
    settings.network.mode = marsa_core::settings::NetworkMode::Auto;
    assert_eq!(Settings::from_toml(&configured).unwrap(), settings);

    let stopped = timeout(WAIT, engine.stop(action(3)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
    assert_eq!(mode(&home, None).await, expected("auto", None, false));
    recovery::signal_running_daemon(&mut process).await;
}

fn expected(configured: &str, active: Option<&str>, restart_pending: bool) -> Value {
    json!({
        "schema_version": 1,
        "kind": "network_mode",
        "configured_mode": configured,
        "active_mode": active,
        "restart_pending": restart_pending,
    })
}

async fn mode(home: &Path, requested: Option<&str>) -> Value {
    let cli = env::var_os("MARSA_TEST_CLI").expect("set MARSA_TEST_CLI to the built Marsa CLI");
    let mut command = tokio::process::Command::new(cli);
    let output = timeout(
        WAIT,
        command
            .args(["network", "mode", "--json"])
            .args(requested)
            .env("HOME", home)
            .env("LC_ALL", "C")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}
