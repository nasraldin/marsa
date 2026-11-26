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
    let response = daemon_info(socket).await;
    assert!(response["HttpProxy"] == APPROVED_HTTP);
    assert!(response["HttpsProxy"] == APPROVED_HTTPS);
}

#[tokio::test]
#[ignore = "requires signed Apple silicon virtualization and the prepared guest-local proxy fixture"]
async fn approved_activation_carries_registry_and_buildkit_requests_through_guest_proxy() {
    use marsa_proto::host::v1::{
        ActivateProxyRequest, PrepareProxyActivationRequest, ProxyActivationOutcome,
        network_client::NetworkClient,
    };

    let source = env::var_os("MARSA_VZ_PROXY_FIXTURE_DIR")
        .expect("prepare the guest-local proxy fixture before running the traffic test");
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir_all(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    private_file(&paths.config_file(), settings.to_toml().unwrap().as_bytes());

    let (daemon, _) = support::stage_bundle_from(scratch.path(), Path::new(&source));
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
    let disk = disk_identity(&paths.data_disk());
    assert!(!paths.state_dir().join("daemon-proxy.json").exists());

    let proxy = "http://127.0.0.1:43128";
    settings.network.proxy.https = Some(proxy.into());
    assert!(replace_settings(&channel, &settings, 2).await);
    assert_eq!(daemon_info(&paths.docker_socket()).await["HttpsProxy"], "");
    let mut settings_client = SettingsClient::new(channel.clone());
    let snapshot = settings_client
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut network = NetworkClient::new(channel);
    let preview = network
        .prepare_proxy_activation(PrepareProxyActivationRequest {
            expected_settings_revision: snapshot.revision,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(preview.interrupts_running_engine);
    assert_eq!(preview.potential_interruptions.len(), 4);
    let mut activation = ActivateProxyRequest {
        request_id: action(3).request_id,
        expected_settings_revision: snapshot.revision,
        approval_token: preview.approval_token,
        approved: false,
    };
    assert_eq!(
        network
            .activate_proxy(activation.clone())
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition,
    );
    assert_eq!(daemon_info(&paths.docker_socket()).await["HttpsProxy"], "");
    assert!(!paths.state_dir().join("daemon-proxy.json").exists());

    activation.request_id = action(4).request_id;
    activation.approved = true;
    let committed = timeout(BOOT, network.activate_proxy(activation.clone()))
        .await
        .unwrap()
        .unwrap()
        .into_inner();
    assert_eq!(committed.outcome(), ProxyActivationOutcome::Applied);
    assert_eq!(
        network
            .activate_proxy(activation)
            .await
            .unwrap()
            .into_inner(),
        committed
    );
    assert_eq!(disk_identity(&paths.data_disk()), disk);
    assert!(daemon_info(&paths.docker_socket()).await["HttpsProxy"] == proxy);
    assert!(
        !settings_client
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner()
            .restart_required
    );
    let persisted = fs::metadata(paths.state_dir().join("daemon-proxy.json")).unwrap();
    assert_eq!(persisted.mode() & 0o7777, 0o600);

    let docker_config = scratch.path().join("docker-config");
    let plugins = docker_config.join("cli-plugins");
    fs::create_dir_all(&plugins).unwrap();
    let buildx = env::split_paths(&env::var_os("PATH").expect("PATH is required"))
        .map(|directory| directory.join("docker-buildx"))
        .find(|binary| binary.is_file())
        .expect("put the pinned docker-buildx on PATH")
        .canonicalize()
        .unwrap();
    std::os::unix::fs::symlink(buildx, plugins.join("docker-buildx")).unwrap();
    let build = scratch.path().join("build-context");
    fs::create_dir(&build).unwrap();
    fs::write(
        build.join("Dockerfile"),
        "FROM registry.marsa-proxy.invalid/marsa/build:proof\n",
    )
    .unwrap();

    let buildx = docker(&paths, &docker_config, &["buildx", "inspect", "default"]).await;
    assert!(buildx.lines().any(
        |line| line.starts_with("Driver:") && line.split_whitespace().nth(1) == Some("docker")
    ));
    let pulled = docker(
        &paths,
        &docker_config,
        &["pull", "registry.marsa-proxy.invalid/marsa/pull:proof"],
    )
    .await;
    assert!(pulled.contains("Digest: sha256:"));
    let export = format!(
        "type=local,dest={}",
        scratch.path().join("export").display()
    );
    docker(
        &paths,
        &docker_config,
        &[
            "buildx",
            "build",
            "--builder",
            "default",
            "--pull",
            "--no-cache",
            "--output",
            &export,
            build.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(
        fs::read_to_string(scratch.path().join("export/fixture-marker")).unwrap(),
        "marsa-build-proof\n"
    );

    let console = fs::read_to_string(paths.console_log()).unwrap();
    assert!(
        console
            .matches("m4-21 fixture accepted restricted CONNECT")
            .count()
            >= 2
    );
    assert!(console.contains("m4-21 fixture pull manifest"));
    assert!(console.contains("m4-21 fixture build manifest"));
    assert!(!fs::read_to_string(logs).unwrap().contains(proxy));
    assert_eq!(
        timeout(WAIT, engine.stop(action(5)))
            .await
            .unwrap()
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped
    );
}

async fn daemon_info(socket: &Path) -> serde_json::Value {
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
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn docker(paths: &MarsaPaths, config: &Path, arguments: &[&str]) -> String {
    let output = timeout(
        BOOT,
        tokio::process::Command::new("docker")
            .env_remove("DOCKER_CONTEXT")
            .env_remove("BUILDX_BUILDER")
            .arg("--config")
            .arg(config)
            .arg("--host")
            .arg(format!("unix://{}", paths.docker_socket().display()))
            .args(arguments)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
