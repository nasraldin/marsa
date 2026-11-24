//! Exercise the real CLI and MCP subprocess against a disposable owner daemon.

use super::*;

#[tokio::test]
#[ignore = "build marsa-cli and marsa-mcp; set MARSA_TEST_CLI to the absolute marsa binary path"]
async fn cli_lists_and_reviews_a_live_approval_over_the_owner_socket() {
    use std::{
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        time::SystemTime,
    };

    use marsad::agent_policy::{Effect, GrantScope, GrantStore};
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        process::Command,
    };

    let cli = std::path::PathBuf::from(std::env::var_os("MARSA_TEST_CLI").unwrap());
    assert!(cli.is_absolute());
    let fixture = AgentFixture::with_name("Fixture \u{202e}agent").await;
    let home = fixture.fixture.home.path();
    let paths = MarsaPaths::from_home(home).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(paths.run_dir())
        .unwrap();
    let owner = UnixListener::bind(paths.daemon_socket()).unwrap();
    let delegate = UnixListener::bind(paths.agent_socket()).unwrap();
    for socket in [paths.daemon_socket(), paths.agent_socket()] {
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let api = fixture.fixture.api.clone();
    tokio::spawn(async move { api.serve(owner, std::future::pending()).await.unwrap() });
    let api = fixture.fixture.api.clone();
    tokio::spawn(async move { api.serve_agents(delegate).await.unwrap() });

    let grant = GrantStore::new(paths, nix::unistd::geteuid().as_raw())
        .update(|grants| {
            grants.record_approval(
                GrantScope::resolve("other-fixture-agent", fixture.project.as_ref())?,
                AgentOperation::StopContainer,
                Effect::Ordinary,
                &Settings::default().agents,
                SystemTime::now(),
            )
        })
        .unwrap();

    let run_cli = || {
        let mut command = Command::new(&cli);
        command
            .env("HOME", home)
            .env("LC_ALL", "en_US.UTF-8")
            .kill_on_drop(true);
        command
    };
    let mut mcp = run_cli()
        .args([
            "mcp",
            "--client-id",
            "stdio-fixture",
            "--project",
            &fixture.project,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = mcp.stdin.take().unwrap();
    let mut lines = BufReader::new(mcp.stdout.take().unwrap()).lines();
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"untrusted-name","version":"1"}
    }});
    input
        .write_all(format!("{init}\n").as_bytes())
        .await
        .unwrap();
    let reply: Value = serde_json::from_str(
        &timeout(WAIT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
    input.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"engine_status\",\"arguments\":{}}}\n").await.unwrap();
    let reply: Value = serde_json::from_str(
        &timeout(WAIT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        reply["result"]["structuredContent"]["data"]["state"],
        "ENGINE_STATE_STOPPED"
    );

    let mut events = fixture.fixture.events().await;
    let waiting = fixture.submit(63, TARGET);
    let approval = pending(&mut events).await;
    let output = run_cli()
        .args(["agents", "list", "--json"])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(&fixture.token));
    assert!(!stdout.contains('\u{202e}'));
    let listed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(listed["kind"], "agents");
    assert_eq!(listed["data"]["clients"][0]["client_id"], "fixture-agent");
    assert_eq!(listed["data"]["clients"][1]["client_id"], "stdio-fixture");
    assert_eq!(listed["data"]["grants"][0]["id"], grant.id);
    assert_eq!(listed["data"]["pending_requests"][0]["id"], approval.id);
    assert_eq!(
        listed["data"]["pending_requests"][0]["redacted_arguments"]["container_id"],
        TARGET
    );

    for (answer, approves) in [("no", false), ("yes", true)] {
        let check = Command::new("python3")
            .args(["-c", include_str!("cli_terminal.py")])
            .arg(&cli)
            .args([approval.id.as_str(), TARGET, answer])
            .env("HOME", home)
            .env("LC_ALL", "en_US.UTF-8")
            .kill_on_drop(true)
            .output();
        let output = timeout(Duration::from_secs(12), check)
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !approves {
            assert!(!waiting.is_finished());
        }
    }
    let Some(WireResult::Output(result)) = timeout(WAIT, waiting).await.unwrap().unwrap().result
    else {
        panic!("the CLI-approved request did not reach the fake engine");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&result.data_json).unwrap(),
        json!({"container_id":TARGET, "stopped":true})
    );

    let waiting = fixture.submit(64, TARGET);
    let approval = pending(&mut events).await;
    let deny = run_cli()
        .args(["agents", "deny", &approval.id, "--json"])
        .output()
        .await
        .unwrap();
    assert!(deny.status.success());
    assert_eq!(
        error_code(timeout(WAIT, waiting).await.unwrap().unwrap()),
        "denied"
    );

    let revoke = run_cli()
        .args(["agents", "revoke", &grant.id, "--yes", "--json"])
        .output()
        .await
        .unwrap();
    assert!(revoke.status.success());
    let list = run_cli()
        .args(["agents", "list", "--json"])
        .output()
        .await
        .unwrap();
    let listed: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(listed["data"]["grants"], json!([]));
    assert_eq!(listed["data"]["pending_requests"], json!([]));
    drop(input);
    assert!(timeout(WAIT, mcp.wait()).await.unwrap().unwrap().success());
}
