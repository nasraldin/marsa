//! Exercise the boot selector with an actual IPv4 firewall rule in the disposable native guest.

use super::*;
use marsa_core::settings::NetworkMode as Mode;
use marsa_proto::host::v1::{NetworkStatusRequest, network_client::NetworkClient};

const RULE_MARKER: &str = "marsa-fallback-fixture: native guest TCP/443 blocked";
const IMAGE: &str =
    "busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec";

#[tokio::test]
#[ignore = "requires the disposable native firewall fixture, Apple silicon and registry access"]
async fn native_tcp_firewall_triggers_verified_fallback_and_preserves_pins() {
    let guest = env::var_os("MARSA_VZ_FALLBACK_FIXTURE_DIR")
        .expect("set MARSA_VZ_FALLBACK_FIXTURE_DIR to the prepared guest-fixture directory");
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let home = scratch.path().join("home");
    fs::create_dir(&home).unwrap();
    let paths = MarsaPaths::from_home(&home).unwrap();
    let (daemon, _) = support::stage_bundle_from(scratch.path(), guest.as_ref());
    let log = scratch.path().join("daemon.log");
    let mut process = recovery::spawn(&daemon, &home, &log);
    let channel = connect(&mut process.0, &paths.daemon_socket(), &log).await;
    let mut settings = Settings::default();
    settings.vm.memory = Some(1024 * MIB);
    settings.vm.idle_stop = false;
    settings.disk.data_size_limit = GIB;
    let share = scratch.path().canonicalize().unwrap().join("fallback-dns");
    fs::create_dir(&share).unwrap();
    settings.shares = vec![share.clone()];
    let mut network = NetworkClient::new(channel.clone());
    let mut engine = EngineClient::new(channel.clone());
    for (step, configured, expected) in [
        (1, Mode::Nat, NetworkMode::Nat),
        (4, Mode::Auto, NetworkMode::Userspace),
        (7, Mode::Userspace, NetworkMode::Userspace),
    ] {
        // Keep this fault specific to TCP. Snapshot only the registry's current macOS
        // answers for this boot. The production host still independently verifies DNS,
        // route, proxy policy and TLS before making a fallback decision.
        let answers: std::collections::BTreeSet<_> =
            tokio::net::lookup_host(("registry-1.docker.io.", 443))
                .await
                .unwrap()
                .filter(|endpoint| endpoint.is_ipv4())
                .map(|endpoint| endpoint.ip())
                .collect();
        assert!(!answers.is_empty());
        let mut hosts = String::new();
        for ip in answers {
            hosts.push_str(&format!(
                "{ip} registry-1.docker.io registry-1.docker.io.\n"
            ));
        }
        fs::write(share.join("registry.hosts"), hosts).unwrap();
        settings.network.mode = configured;
        replace_settings(&channel, &settings, step).await;
        let mut events = EventsClient::new(channel.clone())
            .subscribe(SubscribeRequest {})
            .await
            .unwrap()
            .into_inner();
        recovery::start(&channel, &paths, step + 1).await;
        if configured == Mode::Nat {
            // Pinned health runs after Docker readiness. Wait for its observation while
            // checking every active attachment reported for the lifetime of the pin.
            timeout(Duration::from_secs(40), async {
                while let Some(event) = events.message().await.unwrap() {
                    if let Some(Body::Network(observed)) = event.body
                        && observed.active_mode() != NetworkMode::Unspecified
                    {
                        eprintln!("Pinned NAT health event: {observed:?}");
                        assert_eq!(observed.configured_mode(), NetworkMode::Nat);
                        assert_eq!(observed.active_mode(), NetworkMode::Nat, "{observed:?}");
                        if observed.reason.as_deref() == Some("native_tcp") {
                            return;
                        }
                    }
                }
                panic!("event stream ended before pinned NAT health was published");
            })
            .await
            .expect("missing pinned native TCP health event");
        }
        let status = network
            .status(NetworkStatusRequest {})
            .await
            .unwrap()
            .into_inner();
        assert_eq!(status.active_mode(), expected, "{status:?}");
        assert_eq!(
            status.configured_mode(),
            match configured {
                Mode::Nat => NetworkMode::Nat,
                Mode::Auto => NetworkMode::Auto,
                Mode::Userspace => NetworkMode::Userspace,
            }
        );
        let socket = paths.docker_socket();
        match configured {
            Mode::Nat => {
                assert_eq!(status.reason.as_deref(), Some("native_tcp"));
                assert_eq!(status.restart_pending, Some(false));
                let console = fs::read_to_string(paths.console_log()).unwrap();
                assert!(
                    console.contains(RULE_MARKER),
                    "missing native guest firewall rule"
                );
                assert_eq!(
                    recovery::docker(&socket, &["version", "--format", "{{.Server.Version}}"])
                        .await
                        .trim(),
                    "29.10.0"
                );
            }
            Mode::Auto => {
                let reason = "native_tcp_userspace_verified";
                assert_eq!(status.reason.as_deref(), Some(reason));
                let event = active_network(&mut events).await;
                assert_eq!(event.active_mode(), expected, "{event:?}");
                assert_eq!(event.configured_mode(), NetworkMode::Auto);
                assert_eq!(event.reason.as_deref(), Some(reason));
                // The first boot never pulled any layers. Recovery must serve a real registry pull.
                recovery::docker(&socket, &["pull", IMAGE]).await;
            }
            Mode::Userspace => {
                assert_eq!(
                    recovery::docker(
                        &socket,
                        &["run", "--pull=never", "--rm", IMAGE, "echo", "preserved"]
                    )
                    .await
                    .trim(),
                    "preserved"
                );
            }
        }
        eprintln!(
            "Configured {configured:?}, active {:?}, reason {:?}",
            status.active_mode(),
            status.reason
        );
        engine.stop(action(step + 2)).await.unwrap();
    }
    recovery::signal_running_daemon(&mut process).await;
}
