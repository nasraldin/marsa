//! Qualify owner-approved network restarts and rollback on a real VZ guest.

use super::*;
use marsa_core::settings::NetworkMode as ConfigMode;
use marsa_proto::host::v1::{
    ActivateProxyRequest, PrepareProxyActivationRequest, ProxyActivationOutcome,
    network_client::NetworkClient,
};

async fn docker(socket: &Path, args: &[&str]) -> String {
    let cli = env::var_os("MARSA_TEST_DOCKER").expect("set the fixture Docker CLI");
    let output = timeout(
        BOOT,
        tokio::process::Command::new(cli)
            .env_remove("DOCKER_CONTEXT")
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
#[ignore = "requires Apple silicon virtualization and the packaged guest"]
async fn network_activation_commits_registry_policy_and_recovers_from_a_failed_attachment() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let home = temp.path().join("home");
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir_all(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(GIB);
    settings.vm.idle_stop = false;
    settings.network.mode = ConfigMode::Nat;
    settings.network.proxy.follow_system = false;
    settings.disk.data_size_limit = 2 * GIB;
    fs::write(paths.config_file(), settings.to_toml().unwrap()).unwrap();
    fs::set_permissions(paths.config_file(), fs::Permissions::from_mode(0o600)).unwrap();

    let (daemon, _) = stage_bundle(temp.path());
    let app = temp.path().join("Marsa.app");
    // A correctly signed helper which exits immediately makes only userspace
    // startup fail. Native NAT can boot the previous configuration normally.
    fs::copy(
        "/usr/bin/false",
        app.join("Contents/Helpers/marsa-net-helper"),
    )
    .unwrap();
    let entitlements =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/vz-boot/entitlement.plist");
    checked(
        Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--timestamp=none",
                "--entitlements",
            ])
            .arg(entitlements)
            .arg(&app),
    );
    checked(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&app),
    );

    let log = temp.path().join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    recovery::start(&channel, &paths, 1).await;
    let socket = paths.docker_socket();
    const BUSYBOX: &str =
        "busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec";
    let workload = docker(&socket, &["run", "-d", BUSYBOX, "sleep", "600"]).await;
    // Keep the authoritative test answer inside this disposable guest across restarts.
    docker(
        &socket,
        &[
            "network",
            "create",
            "--subnet",
            "172.30.4.0/24",
            "resolver-test",
        ],
    )
    .await;
    docker(&socket, &["run", "-d", "--restart=always", "--network", "resolver-test",
        "--ip", "172.30.4.53", BUSYBOX, "sh", "-ec",
        "echo 'activation.test 192.0.2.42' >/tmp/dns.conf; exec dnsd -c /tmp/dns.conf -i 0.0.0.0",
    ]).await;
    let mut config = SettingsClient::new(channel.clone());
    let mut network = NetworkClient::new(channel.clone());
    for (step, expected) in [
        (2, ProxyActivationOutcome::Applied),
        (5, ProxyActivationOutcome::PriorEngineRestored),
    ] {
        settings.network.registry_mirrors = vec!["https://mirror.example".into()];
        if step == 2 {
            settings.network.dns = vec!["172.30.4.53".parse().unwrap()];
        } else {
            settings.network.mode = ConfigMode::Userspace;
            settings.network.registry_mirrors = vec!["https://candidate.example".into()];
        }
        assert!(replace_settings(&channel, &settings, step).await);
        let snapshot = config
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner();
        let preview = network
            .prepare_network_activation(PrepareProxyActivationRequest {
                expected_settings_revision: snapshot.revision,
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(preview.registry_mirrors, settings.network.registry_mirrors);
        assert_eq!(preview.dns_servers, ["172.30.4.53"]);
        let approval = preview.approval.unwrap();
        assert!(approval.interrupts_running_engine);
        let request = ActivateProxyRequest {
            request_id: action(step + 1).request_id,
            expected_settings_revision: snapshot.revision,
            approval_token: approval.approval_token,
            approved: false,
        };
        assert_eq!(
            network
                .activate_proxy(request.clone())
                .await
                .unwrap_err()
                .code(),
            Code::FailedPrecondition
        );
        if step == 2 {
            assert_eq!(
                docker(&socket, &["inspect", "-f", "{{.State.Running}}", &workload]).await,
                "true"
            );
        }
        let result = timeout(
            BOOT,
            network.activate_proxy(ActivateProxyRequest {
                approved: true,
                request_id: action(step + 2).request_id,
                ..request
            }),
        )
        .await
        .unwrap()
        .unwrap()
        .into_inner();
        assert_eq!(result.outcome(), expected, "{result:?}");
        let info = daemon_proxy::daemon_info(&paths.docker_socket()).await;
        assert_eq!(
            info["RegistryConfig"]["Mirrors"][0],
            "https://mirror.example/"
        );
        let lookup = docker(&socket, &["run", "--rm", "--pull=never", BUSYBOX, "sh", "-ec",
            "for i in 1 2 3 4 5; do nslookup -type=A activation.test && exit 0; sleep 1; done; exit 1",
        ]).await;
        assert!(
            lookup.split_whitespace().any(|word| word == "192.0.2.42"),
            "{lookup}"
        );
        let after = config
            .get(GetSettingsRequest {})
            .await
            .unwrap()
            .into_inner();
        assert_eq!(after.restart_required, step == 5);
        assert_eq!(
            Settings::from_toml(&after.settings_toml).unwrap().network,
            settings.network
        );
    }
    recovery::signal_running_daemon(&mut process).await;
}
