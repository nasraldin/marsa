//! Exercise delegated operations and owner decisions over separate Unix transports.

#[path = "cli_acceptance.rs"]
mod cli_acceptance;

use super::*;
use marsa_proto::host::v1::{
    AgentOperation, AgentOperationRequest, AgentOperationResponse, ApproveRequest, AuditApprover,
    AuditArguments, AuditDecision, AuditResult, DenyRequest, ListAgentAuditRequest,
    ListPendingApprovalsRequest, OpenAgentSessionRequest, PendingApproval,
    agent_operation_response::Result as WireResult, agent_ops_client::AgentOpsClient,
    agents_client::AgentsClient,
};

const TARGET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct AgentFixture {
    fixture: Fixture,
    restricted: Channel,
    token: String,
    project: String,
}

impl AgentFixture {
    async fn new() -> Self {
        Self::with_name("Fixture agent").await
    }

    async fn with_name(client_name: &str) -> Self {
        let inspect_body = format!(r#"{{"Id":"{TARGET}"}}"#);
        let fixture = Fixture::with_docker(
            vec![AgentStep::Send(b"HR".to_vec())],
            vec![
                AgentStep::Expect(
                    format!("GET /v1.57/containers/{TARGET}/json HTTP/1.1\r\nhost: docker\r\n\r\n")
                        .into_bytes(),
                ),
                AgentStep::Send(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{inspect_body}",
                        inspect_body.len()
                    )
                    .into_bytes(),
                ),
                AgentStep::Expect(
                    format!("POST /v1.57/containers/{TARGET}/stop HTTP/1.1\r\n").into_bytes(),
                ),
                AgentStep::Send(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()),
            ],
        )
        .await;
        let path = fixture.home.path().join("agent.sock");
        let socket = UnixListener::bind(&path).unwrap();
        let api = fixture.api.clone();
        tokio::spawn(async move { api.serve_agents(socket).await.unwrap() });
        let restricted = Endpoint::from_static("http://localhost")
            .connect_with_connector(service_fn(move |_| {
                let path = path.clone();
                async move { UnixStream::connect(path).await.map(TokioIo::new) }
            }))
            .await
            .unwrap();
        let project = fixture.home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let session = AgentsClient::new(fixture.channel.clone())
            .open_session(OpenAgentSessionRequest {
                client_id: "fixture-agent".into(),
                client_name: client_name.into(),
                project_path: project.display().to_string(),
                request_id: action(41).request_id,
            })
            .await
            .unwrap()
            .into_inner();
        Self {
            fixture,
            restricted,
            token: session.token,
            project: session.project_path,
        }
    }

    fn auth<T>(&self, mut request: tonic::Request<T>) -> tonic::Request<T> {
        let mut token: tonic::metadata::MetadataValue<_> =
            format!("Bearer {}", self.token).parse().unwrap();
        token.set_sensitive(true);
        request.metadata_mut().insert("authorization", token);
        request
    }

    fn request(&self, id: u32, target: &str) -> AgentOperationRequest {
        AgentOperationRequest {
            request_id: action(id).request_id,
            client_id: "fixture-agent".into(),
            project_path: self.project.clone(),
            operation: AgentOperation::StopContainer.into(),
            arguments_json: serde_json::json!({"container_id": target}).to_string(),
        }
    }

    fn submit(&self, id: u32, target: &str) -> tokio::task::JoinHandle<AgentOperationResponse> {
        let request = self.auth(tonic::Request::new(self.request(id, target)));
        let mut client = AgentOpsClient::new(self.restricted.clone());
        tokio::spawn(async move { client.execute(request).await.unwrap().into_inner() })
    }

    fn owner(&self) -> AgentsClient<Channel> {
        AgentsClient::new(self.fixture.channel.clone())
    }
}

async fn pending(events: &mut Streaming<DaemonEvent>) -> PendingApproval {
    loop {
        if let Some(Body::Approvals(approvals)) = next(events).await.body
            && let Some(approval) = approvals.approvals.into_iter().next()
        {
            return approval;
        }
    }
}

fn error_code(response: AgentOperationResponse) -> String {
    let Some(WireResult::Error(error)) = response.result else {
        panic!("expected a structured error")
    };
    error.code
}

#[tokio::test]
async fn approval_uses_the_reviewed_target_and_restricted_credentials_cannot_approve() {
    let fixture = AgentFixture::new().await;
    let mut events = fixture.fixture.events().await;
    let initial = next(&mut events).await;
    assert!(
        matches!(initial.body, Some(Body::Snapshot(s)) if s.approvals.as_ref().unwrap().approvals.is_empty())
    );
    let pending_call = fixture.submit(42, TARGET);
    let approval = pending(&mut events).await;
    assert_eq!(approval.client_name, "Fixture agent");
    assert_eq!(approval.project_path, fixture.project);
    assert_eq!(approval.affected[0].name, TARGET);
    assert_eq!(
        approval.redacted_arguments_json,
        fixture.request(42, TARGET).arguments_json
    );
    assert!(!pending_call.is_finished());

    let approve = ApproveRequest {
        request_id: action(43).request_id,
        approval_id: approval.id.clone(),
        grant_for_project: false,
    };
    assert_eq!(
        AgentsClient::new(fixture.restricted.clone())
            .approve(fixture.auth(tonic::Request::new(approve.clone())))
            .await
            .unwrap_err()
            .code(),
        Code::Unimplemented
    );
    assert_eq!(
        fixture
            .owner()
            .approve(fixture.auth(tonic::Request::new(approve.clone())))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    assert_eq!(
        AgentOpsClient::new(fixture.restricted.clone())
            .execute(fixture.auth(tonic::Request::new(fixture.request(42, OTHER))))
            .await
            .unwrap_err()
            .code(),
        Code::AlreadyExists
    );

    let mut owner = fixture.owner();
    owner.approve(approve.clone()).await.unwrap();
    owner.approve(approve).await.unwrap();
    let result = timeout(WAIT, pending_call).await.unwrap().unwrap();
    let Some(WireResult::Output(output)) = result.result else {
        panic!("reviewed fake engine command did not execute")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output.data_json).unwrap(),
        serde_json::json!({"container_id": TARGET, "stopped": true})
    );
    assert!(
        owner
            .list_pending_approvals(ListPendingApprovalsRequest {})
            .await
            .unwrap()
            .into_inner()
            .approvals
            .is_empty()
    );
}

#[tokio::test]
async fn denied_expired_or_out_of_scope_operations_do_not_start_the_engine() {
    let fixture = AgentFixture::new().await;
    let mut events = fixture.fixture.events().await;
    let mut client = AgentOpsClient::new(fixture.restricted.clone());
    let mut forged = fixture.request(44, TARGET);
    forged.client_id = "forged".into();
    assert_eq!(
        client.execute(forged.clone()).await.unwrap_err().code(),
        Code::Unauthenticated
    );
    assert_eq!(
        client
            .execute(fixture.auth(tonic::Request::new(forged)))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    let mut forged = fixture.request(45, TARGET);
    forged.project_path = "/other-project".into();
    assert_eq!(
        client
            .execute(fixture.auth(tonic::Request::new(forged)))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

    let call = fixture.submit(46, TARGET);
    let approval = pending(&mut events).await;
    fixture
        .owner()
        .deny(DenyRequest {
            request_id: action(47).request_id,
            approval_id: approval.id,
        })
        .await
        .unwrap();
    assert_eq!(error_code(call.await.unwrap()), "denied");

    let call = fixture.submit(48, TARGET);
    let approval = pending(&mut events).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::time::resume();
    assert_eq!(
        error_code(timeout(WAIT, call).await.unwrap().unwrap()),
        "approval_timed_out"
    );
    assert_eq!(
        fixture
            .owner()
            .approve(ApproveRequest {
                request_id: action(49).request_id,
                approval_id: approval.id,
                grant_for_project: false
            })
            .await
            .unwrap_err()
            .code(),
        Code::NotFound
    );
    assert_eq!(
        EngineClient::new(fixture.fixture.channel.clone())
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped
    );
    let log = fixture
        .owner()
        .list_audit(ListAgentAuditRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(log.records.len(), 5);
    assert_eq!(log.records[0].decision(), AuditDecision::Expired);
    assert_eq!(log.records[0].result(), AuditResult::TimedOut);
    assert_eq!(log.records[0].approver(), AuditApprover::None);
    assert_eq!(log.records[1].decision(), AuditDecision::Denied);
    assert_eq!(log.records[1].result(), AuditResult::Denied);
    assert_eq!(log.records[1].approver(), AuditApprover::LocalOwner);
    let unauthenticated = log
        .records
        .iter()
        .find(|record| record.result() == AuditResult::Unauthenticated)
        .unwrap();
    assert!(unauthenticated.client_ref.is_none());
}

#[tokio::test]
async fn delegated_container_creation_is_refused_before_mounts_can_reach_docker() {
    let fixture = AgentFixture::new().await;
    let mut request = fixture.request(50, TARGET);
    request.operation = AgentOperation::RunContainer.into();
    request.arguments_json =
        r#"{"bind_mounts":[{"source":"/fixture/host","target":"/work"}]}"#.into();
    let response = AgentOpsClient::new(fixture.restricted.clone())
        .execute(fixture.auth(tonic::Request::new(request)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(error_code(response), "unsupported_operation");
    assert!(
        fixture
            .owner()
            .list_pending_approvals(ListPendingApprovalsRequest {})
            .await
            .unwrap()
            .into_inner()
            .approvals
            .is_empty()
    );
    assert_eq!(
        EngineClient::new(fixture.fixture.channel.clone())
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped
    );
}

#[tokio::test]
async fn replacing_the_registered_project_invalidates_a_pending_approval() {
    let fixture = AgentFixture::new().await;
    let mut events = fixture.fixture.events().await;
    let call = fixture.submit(51, TARGET);
    let approval = pending(&mut events).await;
    let project = std::path::Path::new(&fixture.project);
    std::fs::rename(project, project.with_file_name("old-project")).unwrap();
    std::fs::create_dir(project).unwrap();
    fixture
        .owner()
        .approve(ApproveRequest {
            request_id: action(52).request_id,
            approval_id: approval.id,
            grant_for_project: false,
        })
        .await
        .unwrap();
    assert_eq!(
        error_code(timeout(WAIT, call).await.unwrap().unwrap()),
        "scope_changed"
    );
    assert_eq!(
        EngineClient::new(fixture.fixture.channel.clone())
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Stopped,
    );
}

#[tokio::test]
async fn configured_operate_denial_keeps_read_calls_available() {
    let fixture = AgentFixture::new().await;
    let mut settings = SettingsClient::new(fixture.fixture.channel.clone());
    let snapshot = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut document = Settings::from_toml(&snapshot.settings_toml).unwrap();
    document.agents.operate_approval = marsa_core::settings::OperateApproval::Deny;
    settings
        .update(UpdateSettingsRequest {
            request_id: action(53).request_id,
            expected_revision: snapshot.revision,
            settings_toml: document.to_toml().unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(
        error_code(fixture.submit(54, TARGET).await.unwrap()),
        "policy_refused"
    );
    assert!(
        fixture
            .owner()
            .list_pending_approvals(ListPendingApprovalsRequest {})
            .await
            .unwrap()
            .into_inner()
            .approvals
            .is_empty()
    );
    let mut read = fixture.request(55, TARGET);
    read.operation = AgentOperation::EngineStatus.into();
    read.arguments_json = "{}".into();
    let result = AgentOpsClient::new(fixture.restricted.clone())
        .execute(fixture.auth(tonic::Request::new(read)))
        .await
        .unwrap()
        .into_inner();
    let Some(WireResult::Output(output)) = result.result else {
        panic!("read policy was not applied")
    };
    let data: serde_json::Value = serde_json::from_str(&output.data_json).unwrap();
    assert_eq!(data["state"], "ENGINE_STATE_STOPPED");
}

#[tokio::test]
async fn refused_agent_call_has_one_audit_entry_without_request_secrets() {
    let fixture = AgentFixture::new().await;
    let mut request = fixture.request(56, TARGET);
    request.operation = AgentOperation::RunContainer.into();
    request.arguments_json = r#"{"env":["AWS_SECRET_ACCESS_KEY=fixture-secret-never-log"],"password":"another-fixture-secret"}"#.into();
    let response = AgentOpsClient::new(fixture.restricted.clone())
        .execute(fixture.auth(tonic::Request::new(request)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(error_code(response), "unsupported_operation");
    let log = fixture
        .fixture
        .home
        .path()
        .join(".marsa/state/agent-audit.jsonl");
    let records = std::fs::read_to_string(log).expect("refused call must be recorded");
    assert_eq!(records.lines().count(), 1);
    for secret in [
        "AWS_SECRET_ACCESS_KEY",
        "fixture-secret-never-log",
        "another-fixture-secret",
    ] {
        assert!(!records.contains(secret));
    }
}

#[tokio::test]
async fn owner_can_page_and_filter_audit_without_exposing_it_to_the_agent() {
    let fixture = AgentFixture::new().await;
    let mut rpc = AgentOpsClient::new(fixture.restricted.clone());
    let mut request = fixture.request(57, TARGET);
    request.operation = AgentOperation::EngineStatus.into();
    request.arguments_json = "{}".into();
    for _ in 0..2 {
        rpc.execute(fixture.auth(tonic::Request::new(request.clone())))
            .await
            .unwrap();
    }
    let mut owner = fixture.owner();
    let filter = ListAgentAuditRequest {
        operation: Some(AgentOperation::EngineStatus.into()),
        decision: Some(AuditDecision::Allowed.into()),
        limit: 1,
        ..Default::default()
    };
    let newest = owner.list_audit(filter.clone()).await.unwrap().into_inner();
    assert_eq!(newest.records.len(), 1);
    assert_eq!(newest.records[0].result(), AuditResult::Succeeded);
    assert_eq!(newest.records[0].arguments(), AuditArguments::Empty);
    let client_ref = newest.records[0].client_ref.clone().unwrap();
    assert_eq!(client_ref.len(), 64);
    let prior = owner
        .list_audit(ListAgentAuditRequest {
            client_ref: Some(client_ref),
            cursor: newest.next_cursor,
            ..filter
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(prior.records.len(), 1);
    assert_ne!(prior.records[0].id, newest.records[0].id);
    assert!(prior.next_cursor.is_none());
    assert_eq!(
        AgentsClient::new(fixture.restricted.clone())
            .list_audit(fixture.auth(tonic::Request::new(ListAgentAuditRequest::default())))
            .await
            .unwrap_err()
            .code(),
        Code::Unimplemented
    );
    assert_eq!(
        owner
            .list_audit(fixture.auth(tonic::Request::new(ListAgentAuditRequest::default())))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
}

#[tokio::test]
async fn a_disconnected_agent_call_still_has_one_terminal_record() {
    let fixture = AgentFixture::new().await;
    let mut events = fixture.fixture.events().await;
    let call = fixture.submit(58, TARGET);
    let approval = pending(&mut events).await;
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    let mut owner = fixture.owner();
    owner
        .deny(DenyRequest {
            request_id: action(59).request_id,
            approval_id: approval.id,
        })
        .await
        .unwrap();
    let completed = timeout(WAIT, async {
        loop {
            let page = owner
                .list_audit(ListAgentAuditRequest::default())
                .await
                .unwrap()
                .into_inner();
            if !page.records.is_empty() {
                break page.records;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].result(), AuditResult::Denied);
    assert_eq!(
        completed[0].arguments(),
        AuditArguments::ContainerTargetRedacted
    );
    assert_eq!(completed[0].approver(), AuditApprover::LocalOwner);
}

#[tokio::test]
async fn client_list_exposes_only_unexpired_owner_registrations() {
    use marsa_proto::host::v1::ListAgentClientsRequest;

    let fixture = AgentFixture::new().await;
    let mut owner = fixture.owner();
    owner
        .open_session(OpenAgentSessionRequest {
            client_id: "fixture-agent".into(),
            client_name: "Fixture agent".into(),
            project_path: fixture.project.clone(),
            request_id: action(60).request_id,
        })
        .await
        .unwrap();
    let list = owner
        .list_clients(ListAgentClientsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.clients.len(), 1);
    assert_eq!(list.clients[0].client_id, "fixture-agent");
    assert_eq!(list.clients[0].client_name, "Fixture agent");
    assert_eq!(list.clients[0].project_path, fixture.project);
    assert_eq!(
        owner
            .list_clients(fixture.auth(tonic::Request::new(ListAgentClientsRequest {})))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );
    assert_eq!(
        AgentsClient::new(fixture.restricted.clone())
            .list_clients(ListAgentClientsRequest {})
            .await
            .unwrap_err()
            .code(),
        Code::Unimplemented
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(12 * 60 * 60 + 1)).await;
    tokio::time::resume();
    assert!(
        owner
            .list_clients(ListAgentClientsRequest {})
            .await
            .unwrap()
            .into_inner()
            .clients
            .is_empty()
    );
}
