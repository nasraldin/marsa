//! Exercise Docker authorization and the owner RPC against real Apple shares.

use super::*;
use marsa_proto::host::v1::{
    BindApproval, BindApprovalDecision, ListSharesRequest, ResolveBindRequest,
    shares_client::SharesClient,
};

#[tokio::test]
#[ignore = "requires Apple silicon virtualization, built guest artifacts, Docker CLI and Buildx"]
async fn packaged_bind_approval_limits_access_to_the_explicit_vm_session() {
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let home = root.join("home");
    let approved = root.join("approved");
    let requested = root.join("requested");
    let rejected = root.join("rejected");
    for directory in [&home, &approved, &requested, &rejected] {
        fs::create_dir(directory).unwrap();
    }
    let paths = MarsaPaths::from_home(&home).unwrap();
    fs::create_dir(paths.root()).unwrap();
    fs::set_permissions(paths.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = Settings::default();
    settings.vm.memory = Some(1536 * MIB);
    settings.vm.idle_stop = false;
    settings.network.mode = marsa_core::settings::NetworkMode::Userspace;
    settings.shares = vec![approved.clone()];
    let config = fs::File::create(paths.config_file()).unwrap();
    config
        .set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    (&config)
        .write_all(settings.to_toml().unwrap().as_bytes())
        .unwrap();
    let (daemon, _) = stage_bundle(&root);
    let logs = root.join("daemon.log");
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
    let mut events = EventsClient::new(channel.clone())
        .subscribe(SubscribeRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut engine = EngineClient::new(channel.clone());
    let mut shares = SharesClient::new(channel);
    let result = timeout(BOOT, engine.start(action(1))).await.unwrap();
    assert_eq!(
        result
            .unwrap_or_else(|error| panic!(
                "{error}\n{}\n{}",
                fs::read_to_string(&logs).unwrap(),
                fs::read_to_string(paths.console_log()).unwrap_or_default()
            ))
            .into_inner()
            .state(),
        EngineState::Ready,
    );

    docker(&root, &paths, "build", &approved).await;
    docker(&root, &paths, "allow", &approved).await;
    assert_owner(&approved);
    docker(&root, &paths, "deny", &rejected).await;
    let denial = prompt(&mut events, &rejected).await;
    assert!(!resolve(&mut shares, denial, BindApprovalDecision::Deny, 2).await);
    docker(&root, &paths, "deny", &requested).await;
    let once = prompt(&mut events, &requested).await;
    assert!(resolve(&mut shares, once, BindApprovalDecision::Once, 3).await);
    assert_eq!(
        timeout(BOOT, engine.restart(action(4)))
            .await
            .unwrap()
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Ready
    );
    docker(&root, &paths, "allow", &requested).await;
    assert_owner(&requested);
    let configured = shares
        .list(ListSharesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(configured.shares.len(), 1);
    assert_eq!(configured.shares[0].host_path, approved.to_str().unwrap());
    assert_eq!(
        timeout(BOOT, engine.restart(action(5)))
            .await
            .unwrap()
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Ready
    );
    docker(&root, &paths, "deny", &requested).await;
    let persist = prompt(&mut events, &requested).await;
    assert!(resolve(&mut shares, persist, BindApprovalDecision::Persist, 6).await);
    assert_eq!(
        shares
            .list(ListSharesRequest {})
            .await
            .unwrap()
            .into_inner()
            .shares
            .len(),
        2
    );
    assert_eq!(
        timeout(WAIT, engine.stop(action(7)))
            .await
            .unwrap()
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped
    );
}

async fn prompt(events: &mut Streaming<DaemonEvent>, source: &Path) -> BindApproval {
    timeout(WAIT, async {
        while let Some(event) = events.message().await.unwrap() {
            if let Some(Body::BindApprovals(list)) = event.body
                && let Some(approval) = list
                    .approvals
                    .into_iter()
                    .find(|approval| approval.directories == [source.to_str().unwrap()])
            {
                return approval;
            }
        }
        panic!("bind approval stream closed");
    })
    .await
    .unwrap()
}

async fn resolve(
    shares: &mut SharesClient<Channel>,
    approval: BindApproval,
    decision: BindApprovalDecision,
    step: u32,
) -> bool {
    shares
        .resolve_bind(ResolveBindRequest {
            request_id: action(step).request_id,
            approval_id: approval.id,
            decision: decision.into(),
        })
        .await
        .unwrap()
        .into_inner()
        .requires_restart
}

async fn docker(root: &Path, paths: &MarsaPaths, mode: &str, source: &Path) {
    let artifacts = PathBuf::from(env::var_os("MARSA_VZ_GUEST_DIR").unwrap());
    let result = tokio::process::Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/packaged_runtime/bind_shares.py"))
        .arg(root)
        .arg(paths.docker_socket())
        .arg(artifacts.join("image-rootfs.tar"))
        .arg(mode)
        .arg(source)
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{mode}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn assert_owner(directory: &Path) {
    let written = directory.join("created-from-guest");
    assert_eq!(fs::read_to_string(&written).unwrap(), "marsa-bind-owner\n");
    assert_eq!(
        written.metadata().unwrap().uid(),
        nix::unistd::geteuid().as_raw()
    );
}
