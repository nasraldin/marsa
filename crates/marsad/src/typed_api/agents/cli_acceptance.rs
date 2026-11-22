//! Run the packaged CLI entry points using an explicitly in-process typed daemon fixture.

use std::{
    io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::Stdio,
    time::{Duration, SystemTime},
};

use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::host::v1::PermissionTier;
use serde_json::Value;
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::UnixListener,
    process::{ChildStdout, Command},
    sync::oneshot,
    time::timeout,
};

use super::*;
use crate::agent_policy::Effect;

const WAIT: Duration = Duration::from_secs(6);
const TARGET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Fixture {
    api: TypedApi,
    cli: PathBuf,
    home: TempDir,
    project: String,
}

impl Fixture {
    fn new() -> Self {
        let cli = PathBuf::from(std::env::var_os("MARSA_TEST_CLI").unwrap());
        assert!(cli.is_absolute());
        let home = TempDir::new_in("/tmp").unwrap();
        let paths = MarsaPaths::from_home(home.path()).unwrap();
        for directory in [paths.root(), &paths.run_dir(), &paths.state_dir()] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .unwrap();
        }
        let project = home.path().join("project with spaces");
        std::fs::create_dir(&project).unwrap();
        let project = project
            .canonicalize()
            .unwrap()
            .into_os_string()
            .into_string()
            .unwrap();
        // No supervisor or engine implementation is attached to this typed daemon.
        let api = TypedApi::new(
            paths.clone(),
            nix::unistd::geteuid().as_raw(),
            Settings::default(),
        )
        .unwrap();
        let owner = UnixListener::bind(paths.daemon_socket()).unwrap();
        let delegate = UnixListener::bind(paths.agent_socket()).unwrap();
        for socket in [paths.daemon_socket(), paths.agent_socket()] {
            std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let server = api.clone();
        tokio::spawn(async move { server.serve(owner, std::future::pending()).await.unwrap() });
        let server = api.clone();
        tokio::spawn(async move { server.serve_agents(delegate).await.unwrap() });
        Self {
            api,
            cli,
            home,
            project,
        }
    }

    fn command(&self, mut command: Command) -> Command {
        command
            .env("HOME", self.home.path())
            .env("LC_ALL", "en_US.UTF-8")
            .kill_on_drop(true);
        command
    }

    async fn cli_json(&self, args: &[&str]) -> Value {
        let output = timeout(
            WAIT,
            self.command(Command::new(&self.cli)).args(args).output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(output.status.success(), "CLI failed with {}", output.status);
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn pending(&self) -> (String, oneshot::Receiver<Outcome>) {
        // Only the owner RPCs can resolve this typed fixture. No AgentOps handler
        // can enqueue it, and resolving it yields a decision without execution.
        self.api
            .inner
            .events
            .request_approval(
                PendingApproval {
                    client_id: "fixture-agent".into(),
                    client_name: "Fixture \u{202e}agent".into(),
                    project_path: self.project.clone(),
                    operation: AgentOperation::StopContainer.into(),
                    tier: PermissionTier::Operate.into(),
                    redacted_arguments_json: json!({"container_id": TARGET}).to_string(),
                    affected: vec![AffectedResource {
                        kind: "container".into(),
                        name: TARGET.into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Instant::now() + Duration::from_secs(60),
            )
            .unwrap()
    }
}

async fn rpc_next(lines: &mut Lines<BufReader<ChildStdout>>) -> Value {
    serde_json::from_str(
        &timeout(WAIT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "build marsa-cli and marsa-mcp; set MARSA_TEST_CLI to the absolute marsa binary path"]
async fn cli_mcp_read_and_revoke_from_a_private_home() -> io::Result<()> {
    let fixture = Fixture::new();
    let grant = fixture
        .api
        .inner
        .grants
        .update(|grants| {
            grants.record_approval(
                GrantScope::resolve("fixture-agent", fixture.project.as_ref())?,
                AgentOperation::StopContainer,
                Effect::Ordinary,
                &Settings::default().agents,
                SystemTime::now(),
            )
        })
        .unwrap();

    let mut mcp = fixture
        .command(Command::new(&fixture.cli))
        .args([
            "mcp",
            "--client-id",
            "stdio-fixture",
            "--project",
            &fixture.project,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut input = mcp.stdin.take().unwrap();
    let mut lines = BufReader::new(mcp.stdout.take().unwrap()).lines();
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"untrusted-name","version":"1"}
    }});
    input
        .write_all(format!("{initialize}\n").as_bytes())
        .await?;
    assert_eq!(
        rpc_next(&mut lines).await["result"]["protocolVersion"],
        "2025-11-25"
    );
    for request in [
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"stop_container","arguments":{"container_id":TARGET}
        }}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
            "name":"engine_status","arguments":{}
        }}),
    ] {
        input.write_all(format!("{request}\n").as_bytes()).await?;
    }
    let response = rpc_next(&mut lines).await;
    let tools = response["result"]["tools"].as_array().unwrap();
    assert_eq!(response["id"], 2);
    assert!(tools.iter().any(|tool| tool["name"] == "engine_status"));
    assert!(
        tools
            .iter()
            .all(|tool| tool["annotations"]["readOnlyHint"] == true)
    );
    assert!(!tools.iter().any(|tool| tool["name"] == "stop_container"));
    let response = rpc_next(&mut lines).await;
    assert_eq!(response["id"], 3);
    assert_eq!(response["error"]["code"], -32602);
    let response = rpc_next(&mut lines).await;
    assert_eq!(response["id"], 4);
    assert_eq!(
        response["result"]["structuredContent"]["data"]["state"],
        "ENGINE_STATE_STOPPED"
    );

    let list = fixture.cli_json(&["agents", "list", "--json"]).await;
    assert_eq!(list["kind"], "agents");
    assert_eq!(
        list["data"]["clients"],
        json!([{
            "client_id":"stdio-fixture","client_name":"stdio-fixture","project_path":fixture.project
        }])
    );
    assert_eq!(list["data"]["grants"][0]["id"], grant.id);
    assert_eq!(list["data"]["pending_requests"], json!([]));
    let revoked = fixture
        .cli_json(&["agents", "revoke", &grant.id, "--yes", "--json"])
        .await;
    assert_eq!(revoked["data"]["decision"], "revoked");
    assert_eq!(
        fixture.cli_json(&["agents", "list", "--json"]).await["data"]["grants"],
        json!([])
    );
    drop(input);
    assert!(timeout(WAIT, mcp.wait()).await.unwrap()?.success());
    Ok(())
}

#[tokio::test]
#[ignore = "build marsa-cli; set MARSA_TEST_CLI to the absolute marsa binary path"]
async fn cli_approval_review_answers_an_in_process_typed_fixture() {
    let fixture = Fixture::new();
    let (approval, mut decision) = fixture.pending();
    let list = fixture.cli_json(&["agents", "list", "--json"]).await;
    assert_eq!(list["data"]["pending_requests"][0]["id"], approval);
    assert_eq!(
        list["data"]["pending_requests"][0]["redacted_arguments"]["container_id"],
        TARGET
    );

    for answer in ["no", "yes"] {
        let command = fixture
            .command(Command::new("python3"))
            .args(["-c", include_str!("cli_terminal.py")])
            .arg(&fixture.cli)
            .args([approval.as_str(), TARGET, answer])
            .output();
        let output = timeout(Duration::from_secs(12), command)
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if answer == "no" {
            assert!(matches!(
                decision.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
        }
    }
    assert_eq!(
        timeout(WAIT, decision).await.unwrap().unwrap(),
        Outcome::Approved
    );

    let (approval, decision) = fixture.pending();
    let response = fixture
        .cli_json(&["agents", "deny", &approval, "--json"])
        .await;
    assert_eq!(response["data"]["decision"], "denied");
    assert_eq!(
        timeout(WAIT, decision).await.unwrap().unwrap(),
        Outcome::Denied
    );
    assert!(
        fixture
            .api
            .inner
            .events
            .pending_approvals()
            .approvals
            .is_empty()
    );
}
