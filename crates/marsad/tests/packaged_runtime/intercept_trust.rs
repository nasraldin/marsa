//! Exercise intercepted OCI pulls, explicit trust and live proxy changes in an isolated account.
//! The operator supplies a disposable trust driver as described in proxy_fixture/README.md.

use super::*;
use marsa_proto::host::v1::{
    ActivateProxyRequest, PrepareProxyActivationRequest, ProxyActivationOutcome,
    network_client::NetworkClient,
};

async fn activate(channel: &Channel, settings: &mut Settings, port: u16, step: u32) {
    let proxy = format!("http://127.0.0.1:{port}");
    settings.network.proxy.https = Some(proxy);
    replace_settings(channel, settings, step).await;
    let revision = SettingsClient::new(channel.clone())
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner()
        .revision;
    let mut network = NetworkClient::new(channel.clone());
    let plan = network
        .prepare_proxy_activation(PrepareProxyActivationRequest {
            expected_settings_revision: revision,
        })
        .await
        .unwrap()
        .into_inner();
    let result = timeout(
        BOOT,
        network.activate_proxy(ActivateProxyRequest {
            request_id: action(step + 1).request_id,
            expected_settings_revision: revision,
            approval_token: plan.approval_token,
            approved: true,
        }),
    )
    .await
    .unwrap()
    .unwrap()
    .into_inner();
    assert_eq!(result.outcome(), ProxyActivationOutcome::Applied);
}

async fn pull(socket: &Path, repo: &str) -> std::process::Output {
    let config = tempfile::tempdir().unwrap();
    timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(
            env::var_os("MARSA_TEST_DOCKER").unwrap_or_else(|| "docker".into()),
        )
        .env_remove("DOCKER_CONTEXT")
        .env_remove("DOCKER_HOST")
        .env_remove("DOCKER_TLS_VERIFY")
        .arg("--config")
        .arg(config.path())
        .arg("--host")
        .arg(format!("unix://{}", socket.display()))
        .arg("pull")
        .arg(format!("registry.marsa-proxy.invalid/marsa/{repo}:proof"))
        .kill_on_drop(true)
        .output(),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn driver(program: &Path, operation: &str, ca: &Path) {
    let status = timeout(
        Duration::from_secs(180),
        tokio::process::Command::new(program)
            .args([operation])
            .arg(ca)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        status.success(),
        "disposable trust driver {operation} failed"
    );
}

async fn require_pending(channel: &Channel) {
    let mut api = SettingsClient::new(channel.clone());
    timeout(Duration::from_secs(30), async {
        loop {
            if api
                .get(GetSettingsRequest {})
                .await
                .unwrap()
                .into_inner()
                .restart_required
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("guest did not acknowledge the changed trust set");
}

#[tokio::test]
#[ignore = "requires a disposable trust domain, trust driver and an intercepting proxy guest"]
async fn approved_proxy_uses_keychain_trust_and_replacement_without_reinstall() {
    let driver_path = PathBuf::from(
        env::var_os("MARSA_TEST_DISPOSABLE_TRUST_DRIVER")
            .expect("provide an isolated trust driver"),
    );
    let guest = PathBuf::from(
        env::var_os("MARSA_VZ_INTERCEPT_FIXTURE_DIR").expect("prepare the guest with --intercept"),
    );
    let ca = guest.join("ca.pem");
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let binary = env::var_os("MARSA_TEST_SPLIT_TRUST_DAEMON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_marsad")));
    let (daemon, _) = support::stage_bundle_with_daemon(scratch.path(), &guest, &binary);
    let log = scratch.path().join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    let mut engine = EngineClient::new(channel.clone());
    let mut settings = Settings::default();
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    settings.disk.data_size_limit = GIB;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    settings.network.proxy.follow_system = false;
    replace_settings(&channel, &settings, 1).await;
    recovery::start(&channel, &paths, 2).await;
    let disk = disk_identity(&paths.data_disk());
    activate(&channel, &mut settings, 43129, 3).await;
    let sock = paths.docker_socket();
    let rejected = pull(&sock, "pull").await;
    let message = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        !rejected.status.success() && message.contains("certificate signed by unknown authority"),
        "{message}"
    );
    driver(&driver_path, "trust", &ca).await;
    require_pending(&channel).await;
    // Docker caches its TLS roots. An approved proxy edit restarts it on the same disk.
    activate(&channel, &mut settings, 43130, 5).await;
    assert_eq!(disk_identity(&paths.data_disk()), disk);
    let info = super::daemon_proxy::daemon_info(&sock).await;
    assert_eq!(info["HttpsProxy"], "http://127.0.0.1:43130");
    let accepted = pull(&sock, "pull").await;
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert!(String::from_utf8_lossy(&accepted.stdout).contains("Digest: sha256:"));
    let console = fs::read_to_string(paths.console_log()).unwrap();
    assert!(console.contains("m4-11 fixture intercepted registry TLS"));
    assert!(console.contains("m4-21 fixture pull manifest"));
    driver(&driver_path, "revoke", &ca).await;
    require_pending(&channel).await;
    engine.restart(action(7)).await.unwrap();
    let revoked = pull(&sock, "build").await;
    let message = String::from_utf8_lossy(&revoked.stderr);
    assert!(
        !revoked.status.success() && message.contains("certificate signed by unknown authority"),
        "{message}"
    );
    engine.stop(action(8)).await.unwrap();
    recovery::signal_running_daemon(&mut process).await;
}
