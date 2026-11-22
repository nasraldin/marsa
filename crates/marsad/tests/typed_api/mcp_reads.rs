//! Exercise a delegated read against an exact Docker HTTP fixture.

use super::*;
use marsa_proto::host::v1::{
    AgentOperation, AgentOperationRequest, OpenAgentSessionRequest,
    agent_operation_response::Result as AgentResult, agent_ops_client::AgentOpsClient,
    agents_client::AgentsClient,
};

#[tokio::test]
async fn delegated_container_page_uses_only_the_fixed_read_route() {
    let body =
        br#"[{"Id":"abc","Names":["/fixture"],"Labels":{"credential":"hidden"}},{"Id":"def"}]"#;
    let response = [
        format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .as_bytes(),
        body,
    ]
    .concat();
    let fixture = Fixture::with_docker(
        vec![AgentStep::Send(b"HR".to_vec())],
        vec![
            AgentStep::Expect(b"GET /v1.57/containers/json?all=1&limit=2 HTTP/1.1\r\n".to_vec()),
            AgentStep::Send(response),
        ],
    )
    .await;
    let path = fixture.home.path().join("agent.sock");
    let socket = UnixListener::bind(&path).unwrap();
    let api = fixture.api.clone();
    tokio::spawn(async move { api.serve_agents(socket).await.unwrap() });
    let agent = Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_| {
            let path = path.clone();
            async move { UnixStream::connect(path).await.map(TokioIo::new) }
        }))
        .await
        .unwrap();
    let session = AgentsClient::new(fixture.channel.clone())
        .open_session(OpenAgentSessionRequest {
            client_id: "mcp-fixture".into(),
            client_name: "MCP fixture".into(),
            project_path: fixture.home.path().display().to_string(),
            request_id: action(90).request_id,
        })
        .await
        .unwrap()
        .into_inner();
    let mut request = tonic::Request::new(AgentOperationRequest {
        request_id: action(91).request_id,
        client_id: "mcp-fixture".into(),
        project_path: session.project_path,
        operation: AgentOperation::ListContainers.into(),
        arguments_json: r#"{"limit":1}"#.into(),
    });
    let mut token: tonic::metadata::MetadataValue<_> =
        format!("Bearer {}", session.token).parse().unwrap();
    token.set_sensitive(true);
    request.metadata_mut().insert("authorization", token);
    let response = AgentOpsClient::new(agent)
        .execute(request)
        .await
        .unwrap()
        .into_inner();
    let Some(AgentResult::Output(output)) = response.result else {
        panic!("fake engine read failed")
    };
    assert_eq!(output.next_cursor.as_deref(), Some("1"));
    assert!(output.truncated);
    assert_eq!(
        output.data_json,
        r#"{"items":[{"Id":"abc","Names":["/fixture"]}]}"#
    );
}
