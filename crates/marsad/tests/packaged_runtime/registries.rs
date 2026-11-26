//! Verify the configured registry policy through the running upstream Docker engine.

use super::*;
use marsa_core::settings::NetworkMode as Mode;
use serde_json::Value;

#[tokio::test]
#[ignore = "requires Apple silicon, the packaged guest and the Docker CLI"]
async fn registry_options_reach_dockerd_and_clear_after_restart() {
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
    let mut engine = EngineClient::new(channel.clone());
    let registry = "dev-registry.test:5000";
    let mirror = "https://mirror.test/cache/";
    for (step, mode, active) in [
        (1, Mode::Nat, true),
        (4, Mode::Userspace, true),
        (7, Mode::Userspace, false),
    ] {
        settings.network.mode = mode;
        settings.network.registry_mirrors = if active { vec![mirror.into()] } else { vec![] };
        settings.network.insecure_registries = if active {
            vec![registry.into()]
        } else {
            vec![]
        };
        replace_settings(&channel, &settings, step).await;
        recovery::start(&channel, &paths, step + 1).await;
        let info = recovery::docker(
            &paths.docker_socket(),
            &["info", "--format", "{{json .RegistryConfig}}"],
        )
        .await;
        let info: Value = serde_json::from_str(&info).unwrap();
        if active {
            assert_eq!(info["Mirrors"], serde_json::json!([mirror]));
            assert_eq!(info["IndexConfigs"][registry]["Secure"], false);
        } else {
            assert!(info["Mirrors"].as_array().is_none_or(Vec::is_empty));
            assert!(info["IndexConfigs"].get(registry).is_none());
        }
        engine.stop(action(step + 2)).await.unwrap();
    }
    recovery::signal_running_daemon(&mut process).await;
}
