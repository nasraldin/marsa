//! Exercise owner decisions for an immutable approval fixture with no engine handler.

use std::time::Duration;

use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::host::v1::{AgentOperationRequest, PermissionTier, agent_ops_server::AgentOps};
use tokio::{net::UnixStream, task::yield_now, time::timeout};
use tonic::{Code, transport::server::Connected};

use super::*;

fn owner<T>(data: T) -> Request<T> {
    let (peer, _other) = UnixStream::pair().unwrap();
    let mut request = Request::new(data);
    request.extensions_mut().insert(peer.connect_info());
    request
}

#[tokio::test]
async fn owner_authorizes_the_exact_fixture_and_denial_and_expiry_release_it() {
    let home = tempfile::TempDir::new().unwrap();
    let paths = MarsaPaths::from_home(home.path()).unwrap();
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), Settings::default()).unwrap();
    let project = home.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let response = api
        .open_session(owner(OpenAgentSessionRequest {
            client_id: "fixture".into(),
            client_name: "Approval fixture".into(),
            project_path: project.display().to_string(),
            request_id: "00000000-0000-4000-8000-000000000011".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    let session = api
        .inner
        .agents
        .sessions
        .authenticate(&response.token)
        .unwrap();
    let evidence = r#"{"fixture":"reviewed"}"#;

    for (index, decision, expected_error) in [
        (12, Some(Outcome::Approved), None),
        (13, Some(Outcome::Denied), Some("denied")),
        (14, None, Some("approval_timed_out")),
    ] {
        let service = api.clone();
        let principal = session.clone();
        let mut waiting = tokio::spawn(async move {
            let mut audit_decision = AuditDecision::Refused;
            let result = service
                .allow_operation(
                    &principal,
                    AgentOperation::StopContainer,
                    || {
                        (
                            evidence.into(),
                            vec![AffectedResource {
                                kind: "test".into(),
                                name: "reviewed".into(),
                                ..Default::default()
                            }],
                        )
                    },
                    &mut audit_decision,
                )
                .await;
            (result, audit_decision)
        });
        let approval = timeout(Duration::from_secs(2), async {
            loop {
                let pending = api
                    .list_pending_approvals(owner(ListPendingApprovalsRequest {}))
                    .await
                    .unwrap()
                    .into_inner();
                if let Some(approval) = pending.approvals.into_iter().next() {
                    return approval;
                }
                yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(approval.redacted_arguments_json, evidence);
        assert_eq!(approval.affected[0].name, "reviewed");
        assert_eq!(approval.client_id, "fixture");
        assert_eq!(approval.project_path, response.project_path);
        assert_eq!(approval.tier(), PermissionTier::Operate);
        assert!(!waiting.is_finished());
        let request_id = format!("00000000-0000-4000-8000-{index:012}");
        match decision {
            Some(Outcome::Approved) => {
                let approve = ApproveRequest {
                    request_id,
                    approval_id: approval.id.clone(),
                    grant_for_project: false,
                };
                let mut presented_capability = owner(approve.clone());
                presented_capability
                    .metadata_mut()
                    .insert("authorization", "Bearer fixture".parse().unwrap());
                assert_eq!(
                    api.approve(presented_capability).await.unwrap_err().code(),
                    Code::PermissionDenied
                );
                assert!(!waiting.is_finished());
                for _ in 0..2 {
                    let answer = api
                        .approve(owner(approve.clone()))
                        .await
                        .unwrap()
                        .into_inner();
                    assert_eq!(answer.approval_id, approval.id);
                }
            }
            Some(Outcome::Denied) => {
                let answer = api
                    .deny(owner(DenyRequest {
                        request_id,
                        approval_id: approval.id.clone(),
                    }))
                    .await
                    .unwrap()
                    .into_inner();
                assert_eq!(answer.approval_id, approval.id);
            }
            None => {
                tokio::time::pause();
                tokio::time::advance(queue::LIFETIME + Duration::from_secs(1)).await;
                tokio::time::resume();
                assert_eq!(
                    api.approve(owner(ApproveRequest {
                        request_id,
                        approval_id: approval.id,
                        grant_for_project: false,
                    }))
                    .await
                    .unwrap_err()
                    .code(),
                    Code::NotFound
                );
            }
        }
        let (result, recorded_decision) = timeout(Duration::from_secs(2), &mut waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recorded_decision,
            match decision {
                Some(Outcome::Approved) => AuditDecision::Approved,
                Some(Outcome::Denied) => AuditDecision::Denied,
                None => AuditDecision::Expired,
            }
        );
        match expected_error {
            None => assert!(result.is_ok()),
            Some(expected) => {
                let Some(WireResult::Error(error)) = result.unwrap_err().result else {
                    panic!("expected a structured decision");
                };
                assert_eq!(error.code, expected);
            }
        }
        assert!(api.inner.events.pending_approvals().approvals.is_empty());
    }
}

#[tokio::test]
async fn grant_workers_refuse_factory_exclusive_admission() {
    use std::os::unix::fs::DirBuilderExt;

    let home = tempfile::TempDir::new().unwrap();
    let paths = MarsaPaths::from_home(home.path()).unwrap();
    for directory in [paths.root(), &paths.state_dir()] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .unwrap();
    }
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), Settings::default()).unwrap();
    let grant = api
        .inner
        .grants
        .update(|grants| {
            grants.record_approval(
                GrantScope::resolve("fixture", home.path())?,
                AgentOperation::StopContainer,
                Effect::Ordinary,
                &Settings::default().agents,
                SystemTime::now(),
            )
        })
        .unwrap();

    let exclusive = api.inner.factory_reset.close().await;
    let list = api.list_grants(owner(ListGrantsRequest {})).await;
    let revoke = api
        .revoke_grant(owner(RevokeGrantRequest {
            request_id: "00000000-0000-4000-8000-000000000023".into(),
            grant_id: grant.id.clone(),
        }))
        .await;
    assert_eq!(list.unwrap_err().code(), Code::Unavailable);
    assert_eq!(revoke.unwrap_err().code(), Code::Unavailable);

    drop(exclusive);
    let list = api
        .list_grants(owner(ListGrantsRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.grants, vec![grant]);
}

#[tokio::test]
async fn factory_commit_clears_grants_and_an_existing_delegation_stays_read_only() {
    use std::os::unix::fs::DirBuilderExt;

    use marsa_proto::host::v1::{
        SubscribeRequest, daemon_event::Body, events_server::Events,
        reset_response::Result as ResetResult,
    };
    use tokio_stream::StreamExt;

    let home = tempfile::TempDir::new().unwrap();
    let paths = MarsaPaths::from_home(home.path()).unwrap();
    for directory in [paths.root(), &paths.state_dir(), &paths.vm_dir()] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .unwrap();
    }
    let api = TypedApi::new(paths, nix::unistd::geteuid().as_raw(), Settings::default()).unwrap();
    let registered = api
        .open_session(owner(OpenAgentSessionRequest {
            client_id: "fixture".into(),
            client_name: "Factory fixture".into(),
            project_path: home.path().display().to_string(),
            request_id: "00000000-0000-4000-8000-000000000024".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    api.inner
        .grants
        .update(|grants| {
            grants.record_approval(
                GrantScope::resolve("fixture", home.path())?,
                AgentOperation::StopContainer,
                Effect::Ordinary,
                &Settings::default().agents,
                SystemTime::now(),
            )
        })
        .unwrap();

    let mut stream = api
        .subscribe(owner(SubscribeRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        stream.next().await.unwrap().unwrap().body,
        Some(Body::Snapshot(_))
    ));
    let prepared = api
        .prepare_factory_reset("00000000-0000-4000-8000-000000000025".into())
        .await
        .unwrap();
    let Some(ResetResult::Challenge(challenge)) = prepared.result else {
        panic!("expected factory confirmation challenge");
    };
    let completed = api
        .confirm_factory_reset(
            "00000000-0000-4000-8000-000000000026",
            challenge.confirmation_token,
        )
        .await
        .unwrap();
    assert!(matches!(completed.result, Some(ResetResult::Completed(_))));
    let grants = api
        .list_grants(owner(ListGrantsRequest {}))
        .await
        .unwrap()
        .into_inner()
        .grants;
    assert!(grants.is_empty());
    let published = timeout(Duration::from_secs(2), async {
        while let Some(event) = stream.next().await {
            if let Some(Body::Grants(grants)) = event.unwrap().body {
                return grants;
            }
        }
        panic!("expected a post-reset grants event");
    })
    .await
    .unwrap();
    assert!(published.grants.is_empty());

    for (kind, args, request_id) in [
        (
            AgentOperation::EngineStatus,
            "{}",
            "00000000-0000-4000-8000-000000000027",
        ),
        (
            AgentOperation::StopContainer,
            r#"{"container_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            "00000000-0000-4000-8000-000000000028",
        ),
    ] {
        let mut request = owner(AgentOperationRequest {
            request_id: request_id.into(),
            client_id: "fixture".into(),
            project_path: registered.project_path.clone(),
            operation: kind.into(),
            arguments_json: args.into(),
        });
        let mut credential: tonic::metadata::MetadataValue<_> =
            format!("Bearer {}", registered.token).parse().unwrap();
        credential.set_sensitive(true);
        request.metadata_mut().insert("authorization", credential);
        let response = api.execute(request).await.unwrap().into_inner();
        match (kind, response.result) {
            (AgentOperation::EngineStatus, Some(WireResult::Output(_))) => {}
            (AgentOperation::StopContainer, Some(WireResult::Error(error))) => {
                assert_eq!(error.code, "unsupported_operation");
            }
            _ => panic!("unexpected operation result"),
        }
    }
    assert!(api.inner.events.pending_approvals().approvals.is_empty());
}
