//! Exercise owner decisions for an immutable approval fixture with no engine handler.

use std::time::Duration;

use marsa_core::{paths::MarsaPaths, settings::Settings};
use marsa_proto::host::v1::PermissionTier;
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
            service
                .allow_operation(&principal, AgentOperation::StopContainer, || {
                    (
                        evidence.into(),
                        vec![AffectedResource {
                            kind: "test".into(),
                            name: "reviewed".into(),
                            ..Default::default()
                        }],
                    )
                })
                .await
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
        let result = timeout(Duration::from_secs(2), &mut waiting)
            .await
            .unwrap()
            .unwrap();
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
