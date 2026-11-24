//! Exercise delegated operations and owner decisions over separate Unix transports.

use super::*;
use marsa_proto::host::v1::{
    AgentOperation, AgentOperationRequest, AgentOperationResponse, ApproveRequest, AuditApprover,
<<<<<<< HEAD
    AuditArguments, AuditDecision, AuditResult, DenyRequest, ListAgentAuditRequest,
    ListPendingApprovalsRequest, OpenAgentSessionRequest, PendingApproval,
    agent_operation_response::Result as WireResult, agent_ops_client::AgentOpsClient,
    agents_client::AgentsClient,
=======
    AuditArguments, AuditDecision, AuditResult, ListAgentAuditRequest, ListPendingApprovalsRequest,
    OpenAgentSessionRequest, agent_operation_response::Result as WireResult,
    agent_ops_client::AgentOpsClient, agents_client::AgentsClient,
>>>>>>> origin/main
};

const TARGET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct AgentFixture {
    fixture: Fixture,
    restricted: Channel,
    token: String,
    project: String,
}

impl AgentFixture {
    async fn new() -> Self {
        let fixture = Fixture::with_docker(vec![AgentStep::Send(b"HR".to_vec())], Vec::new()).await;
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
                client_name: "Fixture agent".into(),
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

    fn owner(&self) -> AgentsClient<Channel> {
        AgentsClient::new(self.fixture.channel.clone())
    }
}

fn error_code(response: AgentOperationResponse) -> String {
    let Some(WireResult::Error(error)) = response.result else {
        panic!("expected a structured error")
    };
    error.code
}

#[tokio::test]
async fn delegated_credentials_cannot_decide_or_impersonate_another_scope() {
    let fixture = AgentFixture::new().await;
    let approve = ApproveRequest {
        request_id: action(43).request_id,
        approval_id: "fixture".into(),
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
            .approve(fixture.auth(tonic::Request::new(approve)))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

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
<<<<<<< HEAD

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
=======
>>>>>>> origin/main
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
async fn replacing_the_registered_project_invalidates_delegated_reads() {
    let fixture = AgentFixture::new().await;
    let project = std::path::Path::new(&fixture.project);
    std::fs::rename(project, project.with_file_name("old-project")).unwrap();
    std::fs::create_dir(project).unwrap();
    let mut read = fixture.request(51, TARGET);
    read.operation = AgentOperation::EngineStatus.into();
    read.arguments_json = "{}".into();
    let response = AgentOpsClient::new(fixture.restricted.clone())
        .execute(fixture.auth(tonic::Request::new(read)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(error_code(response), "scope_changed");
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
<<<<<<< HEAD
=======
async fn delegated_exact_id_stop_is_rejected_without_queuing_on_a_running_engine() {
    let fixture = AgentFixture::new().await;
    let mut engine = EngineClient::new(fixture.fixture.channel.clone());
    let ready = engine.start(action(70)).await.unwrap().into_inner();
    assert_eq!(ready.state(), EngineState::Ready);
    let mut delegated = AgentOpsClient::new(fixture.restricted.clone());
    let operation =
        delegated.execute(fixture.auth(tonic::Request::new(fixture.request(71, TARGET))));
    let response = timeout(Duration::from_millis(500), operation)
        .await
        .expect("an unsupported stop must return without waiting for approval")
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
        engine
            .status(EngineStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .state(),
        EngineState::Ready
    );
    let raw = std::fs::read_to_string(
        fixture
            .fixture
            .home
            .path()
            .join(".marsa/state/agent-audit.jsonl"),
    )
    .unwrap();
    assert_eq!(raw.lines().count(), 1);
    for private in [
        TARGET,
        &fixture.project,
        &fixture.token,
        "fixture-agent",
        "container_id",
        &action(71).request_id,
    ] {
        assert!(!raw.contains(private));
    }
    let audit = fixture
        .owner()
        .list_audit(ListAgentAuditRequest::default())
        .await
        .unwrap()
        .into_inner();
    let [receipt] = audit.records.as_slice() else {
        panic!("expected a single refusal receipt")
    };
    assert_eq!(receipt.operation(), AgentOperation::StopContainer);
    assert_eq!(receipt.decision(), AuditDecision::Refused);
    assert_eq!(receipt.result(), AuditResult::Rejected);
    assert_eq!(receipt.approver(), AuditApprover::None);
    assert_eq!(receipt.arguments(), AuditArguments::Omitted);
}

#[tokio::test]
>>>>>>> origin/main
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
<<<<<<< HEAD
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
=======
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
>>>>>>> origin/main
}
