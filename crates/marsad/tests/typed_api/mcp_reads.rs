//! Exercise delegated reads and their redacted audit entries over the private Unix sockets.

use super::*;
use marsa_proto::host::v1::{
    AgentOperation, AgentOperationRequest, AgentOutput, AuditArguments, AuditDecision, AuditResult,
    ListAgentAuditRequest, OpenAgentSessionRequest,
    agent_operation_response::Result as AgentResult, agent_ops_client::AgentOpsClient,
    agents_client::AgentsClient,
};

struct ReadFixture {
    daemon: Fixture,
    delegated: AgentOpsClient<Channel>,
    project: String,
    token: tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
}

impl ReadFixture {
    async fn new(route: &str, body: &[u8]) -> Self {
        let response = [
            format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
            body,
        ]
        .concat();
        let daemon = Fixture::with_docker(
            vec![AgentStep::Send(b"HR".to_vec())],
            vec![
                AgentStep::Expect(format!("GET {route} HTTP/1.1\r\n").into_bytes()),
                AgentStep::Send(response),
            ],
        )
        .await;
        let path = daemon.home.path().join("agent.sock");
        let socket = UnixListener::bind(&path).unwrap();
        let api = daemon.api.clone();
        tokio::spawn(async move { api.serve_agents(socket).await.unwrap() });
        let agent = Endpoint::from_static("http://localhost")
            .connect_with_connector(service_fn(move |_| {
                let path = path.clone();
                async move { UnixStream::connect(path).await.map(TokioIo::new) }
            }))
            .await
            .unwrap();
        let session = AgentsClient::new(daemon.channel.clone())
            .open_session(OpenAgentSessionRequest {
                client_id: "mcp-fixture".into(),
                client_name: "MCP fixture".into(),
                project_path: daemon.home.path().display().to_string(),
                request_id: action(90).request_id,
            })
            .await
            .unwrap()
            .into_inner();
        let mut token: tonic::metadata::MetadataValue<_> =
            format!("Bearer {}", session.token).parse().unwrap();
        token.set_sensitive(true);
        Self {
            daemon,
            delegated: AgentOpsClient::new(agent),
            project: session.project_path,
            token,
        }
    }

    async fn execute(&mut self, operation: AgentOperation, arguments_json: String) -> AgentOutput {
        let mut request = tonic::Request::new(AgentOperationRequest {
            request_id: action(91).request_id,
            client_id: "mcp-fixture".into(),
            project_path: self.project.clone(),
            operation: operation.into(),
            arguments_json,
        });
        request
            .metadata_mut()
            .insert("authorization", self.token.clone());
        let response = self.delegated.execute(request).await.unwrap().into_inner();
        let Some(AgentResult::Output(output)) = response.result else {
            panic!("fake engine read failed")
        };
        output
    }

    async fn assert_redacted(
        &self,
        operation: AgentOperation,
        arguments: AuditArguments,
        omitted: &[&str],
    ) {
        let response = AgentsClient::new(self.daemon.channel.clone())
            .list_audit(ListAgentAuditRequest {
                operation: Some(operation.into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.records.len(), 1);
        let record = &response.records[0];
        assert_eq!(record.operation(), operation);
        assert_eq!(record.arguments(), arguments);
        assert_eq!(record.decision(), AuditDecision::Allowed);
        assert_eq!(record.result(), AuditResult::Succeeded);
        let path = self
            .daemon
            .home
            .path()
            .join(".marsa/state/agent-audit.jsonl");
        let log = std::fs::read_to_string(path).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(omitted.iter().all(|value| !log.contains(value)));
    }
}

#[tokio::test]
async fn delegated_container_page_uses_fixed_read_route_and_audits_only_the_schema() {
    let body = br#"[{"Id":"abc","Names":["/ignore-policy-fixture"],"Labels":{"credential":"hidden-fixture-label"}},{"Id":"def"}]"#;
    let mut fixture = ReadFixture::new("/v1.57/containers/json?all=1&limit=2", body).await;
    let output = fixture
        .execute(
            AgentOperation::ListContainers,
            r#"{"limit":1,"cursor":"000"}"#.into(),
        )
        .await;
    assert_eq!(output.next_cursor.as_deref(), Some("1"));
    assert!(output.truncated);
    assert_eq!(
        output.data_json,
        r#"{"items":[{"Id":"abc","Names":["/ignore-policy-fixture"]}]}"#
    );
    fixture
        .assert_redacted(
            AgentOperation::ListContainers,
            AuditArguments::PageOptionsRedacted,
            &[
                "cursor",
                "\"000\"",
                "hidden-fixture-label",
                "ignore-policy-fixture",
            ],
        )
        .await;
}

#[tokio::test]
async fn delegated_container_inspect_does_not_persist_the_target_or_engine_data() {
    let target = "a".repeat(64);
    let body = serde_json::json!({"Id":target,"Name":"/fixture-private-name","Config":{"Env":["FIXTURE_SECRET=never-persist"]}}).to_string();
    let mut fixture =
        ReadFixture::new(&format!("/v1.57/containers/{target}/json"), body.as_bytes()).await;
    let output = fixture
        .execute(
            AgentOperation::InspectContainer,
            serde_json::json!({"id":target}).to_string(),
        )
        .await;
    assert!(output.data_json.contains("/fixture-private-name"));
    assert!(!output.data_json.contains("FIXTURE_SECRET"));
    fixture
        .assert_redacted(
            AgentOperation::InspectContainer,
            AuditArguments::ContainerTargetRedacted,
            &[
                &target,
                "FIXTURE_SECRET",
                "never-persist",
                "fixture-private-name",
            ],
        )
        .await;
}
