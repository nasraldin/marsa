//! Expose only typed operations on the restricted transport and decisions on the owner transport.

pub(super) mod queue;
mod sessions;

#[cfg(test)]
mod tests;

use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_core::settings::Language;
use marsa_proto::host::v1::{
    AffectedResource, AgentOperation, AgentOperationError, AgentOperationRequest,
    AgentOperationResponse, AgentOutput, ApproveRequest, DecisionResponse, DenyRequest,
    ListGrantsRequest, ListGrantsResponse, ListPendingApprovalsRequest,
    ListPendingApprovalsResponse, OpenAgentSessionRequest, OpenAgentSessionResponse,
    PendingApproval, RevokeGrantRequest, RevokeGrantResponse,
    agent_operation_response::Result as WireResult, agent_ops_server::AgentOps,
    agents_server::Agents,
};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{Instant, timeout_at};
use tonic::{Request, Response, Status};

use super::{TypedApi, requests};
use crate::agent_policy::{Decision, Effect, GrantScope, GrantStore, decide};
use queue::Outcome;
use sessions::Session;

const ARGUMENT_LIMIT: usize = 4096;

pub(super) struct State {
    sessions: sessions::Sessions,
    grants: GrantStore,
    open: requests::Cache<OpenAgentSessionRequest, OpenAgentSessionResponse>,
    decisions: requests::Cache<(String, Outcome), DecisionResponse>,
    revoke: requests::Cache<String, RevokeGrantResponse>,
    grant_admin: tokio::sync::Mutex<()>,
}

impl State {
    pub(super) fn new(paths: marsa_core::paths::MarsaPaths, uid: u32) -> Self {
        Self {
            sessions: sessions::Sessions::default(),
            grants: GrantStore::new(paths, uid),
            open: requests::Cache::new(),
            decisions: requests::Cache::new(),
            revoke: requests::Cache::new(),
            grant_admin: tokio::sync::Mutex::new(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Prepared {
    EngineStatus,
}

impl Prepared {
    fn parse(operation: AgentOperation, args: &str) -> Option<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Empty {}

        if args.len() > ARGUMENT_LIMIT {
            return None;
        }
        match operation {
            AgentOperation::EngineStatus => {
                serde_json::from_str::<Empty>(args).ok()?;
                Some(Self::EngineStatus)
            }
            _ => None,
        }
    }

    const fn operation(&self) -> AgentOperation {
        match self {
            Self::EngineStatus => AgentOperation::EngineStatus,
        }
    }
}

#[tonic::async_trait]
impl AgentOps for TypedApi {
    async fn execute(
        &self,
        request: Request<AgentOperationRequest>,
    ) -> Result<Response<AgentOperationResponse>, Status> {
        let session = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .and_then(|token| self.inner.agents.sessions.authenticate(token));
        let request = self.authorize(request)?;
        let session =
            session.ok_or_else(|| Status::unauthenticated("valid agent session required"))?;
        if request.client_id != session.client_id || request.project_path != session.scope.project()
        {
            return Err(Status::permission_denied(
                "request is outside the registered agent scope",
            ));
        }
        let operation = request.operation();
        // A full ID is not an immutable mutation selector in Docker's HTTP API:
        // the Engine can fall back to a mutable name after a separate inspect.
        if operation != AgentOperation::EngineStatus {
            return Ok(Response::new(self.operation_error(
                "unsupported_operation",
                "This agent operation is unavailable.",
                "عملية الوكيل هذه غير متاحة.",
                None,
            )));
        }
        let Some(operation) = Prepared::parse(operation, &request.arguments_json) else {
            return Ok(Response::new(self.operation_error(
                "invalid_arguments",
                "Invalid agent operation arguments.",
                "وسائط عملية الوكيل غير صالحة.",
                None,
            )));
        };
        let api = self.clone();
        let principal = session.clone();
        let result = session
            .requests
            .run(&request.request_id, operation.clone(), || async move {
                Ok(api.execute_prepared(principal, operation).await)
            })
            .await?;
        Ok(Response::new(result))
    }
}

#[tonic::async_trait]
impl Agents for TypedApi {
    async fn open_session(
        &self,
        request: Request<OpenAgentSessionRequest>,
    ) -> Result<Response<OpenAgentSessionResponse>, Status> {
        let mut input = self.owner_request(request)?;
        let id = std::mem::take(&mut input.request_id);
        let api = self.clone();
        let result = self
            .inner
            .agents
            .open
            .run(&id, input.clone(), || async move {
                if input.client_name.trim().is_empty()
                    || input.client_name.len() > 128
                    || input.client_name.chars().any(char::is_control)
                {
                    return Err(Status::invalid_argument("invalid agent client name"));
                }
                let scope = checked_scope(input.client_id.clone(), input.project_path).await?;
                let project_path = scope.project().to_owned();
                let token =
                    api.inner
                        .agents
                        .sessions
                        .insert(input.client_id, input.client_name, scope)?;
                Ok(OpenAgentSessionResponse {
                    token,
                    project_path,
                })
            })
            .await?;
        Ok(Response::new(result))
    }

    async fn list_pending_approvals(
        &self,
        request: Request<ListPendingApprovalsRequest>,
    ) -> Result<Response<ListPendingApprovalsResponse>, Status> {
        self.owner_request(request)?;
        Ok(Response::new(self.inner.events.pending_approvals()))
    }

    async fn approve(
        &self,
        request: Request<ApproveRequest>,
    ) -> Result<Response<DecisionResponse>, Status> {
        let request = self.owner_request(request)?;
        if request.grant_for_project {
            return Err(Status::invalid_argument(
                "this operation needs individual approval",
            ));
        }
        self.resolve(request.request_id, request.approval_id, Outcome::Approved)
            .await
            .map(Response::new)
    }

    async fn deny(
        &self,
        request: Request<DenyRequest>,
    ) -> Result<Response<DecisionResponse>, Status> {
        let request = self.owner_request(request)?;
        self.resolve(request.request_id, request.approval_id, Outcome::Denied)
            .await
            .map(Response::new)
    }

    async fn list_grants(
        &self,
        request: Request<ListGrantsRequest>,
    ) -> Result<Response<ListGrantsResponse>, Status> {
        self.owner_request(request)?;
        let api = self.clone();
        Ok(Response::new(
            tokio::task::spawn_blocking(move || api.read_grants())
                .await
                .map_err(|_| Status::internal("agent grants worker stopped"))??,
        ))
    }

    async fn revoke_grant(
        &self,
        request: Request<RevokeGrantRequest>,
    ) -> Result<Response<RevokeGrantResponse>, Status> {
        let request = self.owner_request(request)?;
        let api = self.clone();
        let response = self
            .inner
            .agents
            .revoke
            .run(
                &request.request_id,
                request.grant_id.clone(),
                || async move {
                    let _edit = api.inner.agents.grant_admin.lock().await;
                    let admin = api.clone();
                    tokio::task::spawn_blocking(move || {
                        let api = admin;
                        if !api
                            .inner
                            .agents
                            .grants
                            .update(|grants| Ok(grants.revoke(&request.grant_id)))
                            .map_err(|_| Status::internal("agent grants unavailable"))?
                        {
                            return Err(Status::not_found("unknown agent grant"));
                        }
                        api.inner.events.publish_grants(api.read_grants()?);
                        Ok(RevokeGrantResponse {})
                    })
                    .await
                    .map_err(|_| Status::internal("agent grants worker stopped"))?
                },
            )
            .await?;
        Ok(Response::new(response))
    }
}

impl TypedApi {
    fn owner_request<T>(&self, request: Request<T>) -> Result<T, Status> {
        if request.metadata().contains_key("authorization") {
            return Err(Status::permission_denied(
                "agent credentials cannot authorize owner actions",
            ));
        }
        self.authorize(request)
    }

    async fn resolve(
        &self,
        id: String,
        approval_id: String,
        outcome: Outcome,
    ) -> Result<DecisionResponse, Status> {
        let events = self.inner.events.clone();
        self.inner
            .agents
            .decisions
            .run(&id, (approval_id.clone(), outcome), || async move {
                events.resolve_approval(&approval_id, outcome)?;
                Ok(DecisionResponse { approval_id })
            })
            .await
    }

    fn read_grants(&self) -> Result<ListGrantsResponse, Status> {
        let grants = self
            .inner
            .agents
            .grants
            .load()
            .map_err(|_| Status::internal("agent grants unavailable"))?;
        let active = grants
            .active(SystemTime::now())
            .map_err(|_| Status::internal("agent grants unavailable"))?;
        Ok(ListGrantsResponse {
            grants: active.map(crate::agent_policy::Grant::to_wire).collect(),
        })
    }

    async fn execute_prepared(
        &self,
        principal: Arc<Session>,
        operation: Prepared,
    ) -> AgentOperationResponse {
        let result = self
            .allow_operation(&principal, operation.operation(), || {
                ("{}".into(), Vec::new())
            })
            .await;
        if let Err(result) = result {
            return result;
        }
        match operation {
            Prepared::EngineStatus => {
                let status = self.inner.events.engine_status();
                output(json!({"state":status.state().as_str_name(), "backend":status.backend}))
            }
        }
    }

    async fn allow_operation(
        &self,
        principal: &Session,
        kind: AgentOperation,
        details: impl FnOnce() -> (String, Vec<AffectedResource>),
    ) -> Result<(), AgentOperationResponse> {
        if !self.scope_is_current(principal).await {
            return Err(self.project_changed());
        }
        let (settings, _) = self.inner.events.settings();
        match decide(kind, Effect::Ordinary, &settings.agents) {
            Decision::Allow => return Ok(()),
            Decision::Refuse(_) => return Err(self.policy_refusal()),
            Decision::Approve { tier, .. } => {
                let expires = principal.expires.min(Instant::now() + queue::LIFETIME);
                let (redacted_arguments_json, affected) = details();
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                let expires_at_unix_millis = i64::try_from(
                    now.saturating_add(expires.saturating_duration_since(Instant::now()))
                        .as_millis(),
                )
                .unwrap_or(i64::MAX);
                let approval = PendingApproval {
                    id: String::new(),
                    client_id: principal.client_id.clone(),
                    client_name: principal.name.clone(),
                    project_path: principal.scope.project().into(),
                    operation: kind.into(),
                    tier: tier.into(),
                    redacted_arguments_json,
                    affected,
                    expires_at_unix_millis,
                    // Reusable mutations require independently verified project scope.
                    can_grant_for_project: false,
                };
                let (id, answer) = self
                    .inner
                    .events
                    .request_approval(approval, expires)
                    .map_err(|_| {
                        self.operation_error(
                            "approval_unavailable",
                            "The agent approval queue is unavailable. Try again.",
                            "قائمة الموافقات للوكيل غير متاحة. أعد المحاولة.",
                            None,
                        )
                    })?;
                let mut closing = self.inner.closing.subscribe();
                let answer = tokio::select! {
                    biased;
                    _ = closing.wait_for(|closing| *closing) => None,
                    answer = timeout_at(expires, answer) => answer.ok().and_then(Result::ok),
                };
                self.inner.events.discard_approval(&id);
                match answer {
                    Some(Outcome::Approved) => {}
                    Some(Outcome::Denied) => {
                        return Err(self.operation_error(
                            "denied",
                            "The owner denied this request.",
                            "رفض المالك هذا الطلب.",
                            Some("owner_denied"),
                        ));
                    }
                    None => {
                        return Err(self.operation_error(
                            "approval_timed_out",
                            "The request expired before approval.",
                            "انتهت مهلة الطلب قبل الموافقة.",
                            Some("approval_expired"),
                        ));
                    }
                }
            }
        }

        if !self.scope_is_current(principal).await {
            return Err(self.project_changed());
        }
        if principal.expires <= Instant::now()
            || *self.inner.closing.borrow()
            || matches!(
                decide(
                    kind,
                    Effect::Ordinary,
                    &self.inner.events.settings().0.agents
                ),
                Decision::Refuse(_)
            )
        {
            return Err(self.policy_refusal());
        }
        Ok(())
    }

    async fn scope_is_current(&self, principal: &Session) -> bool {
        let current = checked_scope(
            principal.client_id.clone(),
            principal.scope.project().to_owned(),
        )
        .await;
        current.as_ref().ok() == Some(&principal.scope)
    }

    fn project_changed(&self) -> AgentOperationResponse {
        self.operation_error(
            "scope_changed",
            "The agent project changed. Open a new session.",
            "تغيّر مشروع الوكيل. افتح جلسة جديدة.",
            Some("project_changed"),
        )
    }

    fn policy_refusal(&self) -> AgentOperationResponse {
        self.operation_error(
            "policy_refused",
            "Agent policy refused this request.",
            "رفضت سياسة الوكيل هذا الطلب.",
            Some("agent_policy"),
        )
    }

    fn operation_error(
        &self,
        code: &str,
        en: &str,
        ar: &str,
        reason: Option<&str>,
    ) -> AgentOperationResponse {
        let language = self.inner.events.settings().0.ui.language;
        let arabic = language == Language::Arabic
            || (language == Language::System
                && ["LC_ALL", "LC_MESSAGES", "LANG"]
                    .into_iter()
                    .filter_map(|key| std::env::var(key).ok())
                    .find(|value| !value.is_empty())
                    .is_some_and(|value| value.split(['_', '-', '.', '@']).next() == Some("ar")));
        AgentOperationResponse {
            result: Some(WireResult::Error(AgentOperationError {
                code: code.into(),
                message: if arabic { ar } else { en }.into(),
                denial_reason: reason.map(str::to_owned),
            })),
        }
    }
}

async fn checked_scope(client: String, project: String) -> Result<GrantScope, Status> {
    tokio::task::spawn_blocking(move || GrantScope::resolve(&client, Path::new(&project)))
        .await
        .map_err(|_| Status::internal("agent identity worker stopped"))?
        .map_err(|_| Status::invalid_argument("invalid agent client or project"))
}

fn output(data: serde_json::Value) -> AgentOperationResponse {
    AgentOperationResponse {
        result: Some(WireResult::Output(AgentOutput {
            data_json: data.to_string(),
            truncated: false,
            next_cursor: None,
        })),
    }
}
