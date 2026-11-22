//! Read the live daemon proxy without contacting a proxy or registry.

use super::*;

const APPROVED_HTTP: &str = "http://approved-proxy.invalid:3128";
const APPROVED_HTTPS: &str = "http://approved-proxy.invalid:8080";
const DRAFT: &str = "http://draft-proxy.invalid:8181";

#[tokio::test]
#[ignore = "requires Apple silicon virtualization and built guest and network helper artifacts"]
async fn approved_proxy_survives_cold_boots_and_draft_edits() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir_all(paths.state_dir()).unwrap();
    for dir in [paths.root().to_owned(), paths.state_dir()] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let mut settings = Settings::default();
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    let next_settings = settings.to_toml().unwrap();
    private_file(&paths.config_file(), next_settings.as_bytes());
    // Seed the last working state that M4-21's approved transaction will publish.
    let record = serde_json::json!({
        "version": 1, "http": APPROVED_HTTP, "https": APPROVED_HTTPS
    });
    let approved = paths.state_dir().join("daemon-proxy.json");
    private_file(&approved, record.to_string().as_bytes());

    let (daemon, _) = stage_bundle(scratch.path());
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
    let mut engine = EngineClient::new(channel.clone());
    start(&mut engine, &paths, 1).await;
    assert_proxy(&paths.docker_socket()).await;
    let disk = disk_identity(&paths.data_disk());

    settings.network.proxy.http = Some(DRAFT.into());
    assert!(replace_settings(&channel, &settings, 2).await);
    let saved = Settings::from_toml(&fs::read_to_string(paths.config_file()).unwrap()).unwrap();
    assert_eq!(saved.network.proxy.http.as_deref(), Some(DRAFT));
    assert_eq!(fs::read(&approved).unwrap(), record.to_string().as_bytes());
    assert_eq!(
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Ready
    );
    assert_proxy(&paths.docker_socket()).await;

    let stopped = timeout(WAIT, engine.stop(action(3)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
    start(&mut engine, &paths, 4).await;
    assert_proxy(&paths.docker_socket()).await;
    assert_eq!(disk_identity(&paths.data_disk()), disk);
    let pending = SettingsClient::new(channel)
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(pending.restart_required);

    let stopped = timeout(WAIT, engine.stop(action(5)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.into_inner().state(), EngineState::Stopped);
    let log = fs::read_to_string(logs).unwrap();
    assert!(!log.contains("approved-proxy.invalid"));
    assert!(!log.contains("draft-proxy.invalid"));
    let console = fs::read_to_string(paths.console_log()).unwrap();
    assert!(!console.contains("approved-proxy.invalid"));
    assert!(!console.contains("draft-proxy.invalid"));
}

fn private_file(path: &Path, data: &[u8]) {
    let mut file = fs::File::create_new(path).unwrap();
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    file.write_all(data).unwrap();
}

async fn start(engine: &mut EngineClient<Channel>, paths: &MarsaPaths, step: u32) {
    let response = timeout(BOOT, engine.start(action(step))).await.unwrap();
    let state = response
        .unwrap_or_else(|error| {
            panic!(
                "engine start failed {error}\n{}",
                fs::read_to_string(paths.console_log()).unwrap_or_default()
            )
        })
        .into_inner()
        .state();
    assert_eq!(state, EngineState::Ready);
}

async fn assert_proxy(socket: &Path) {
    // Moby 29.10.0 /info reports the proxy used by the running process.
    let output = tokio::process::Command::new("/usr/bin/curl")
        .args([
            "--silent",
            "--fail",
            "--max-time",
            "10",
            "--noproxy",
            "*",
            "--unix-socket",
        ])
        .arg(socket)
        .arg("http://docker/info")
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let response = serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    assert!(response["HttpProxy"] == APPROVED_HTTP);
    assert!(response["HttpsProxy"] == APPROVED_HTTPS);
}
