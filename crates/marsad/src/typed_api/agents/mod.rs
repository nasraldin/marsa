//! Expose only typed operations on the restricted transport and decisions on the owner transport.

mod attempt;
mod audit;
mod engine;
pub(super) mod queue;
mod reads;
mod sessions;

use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_core::settings::Language;
use marsa_proto::host::v1::{
    AffectedResource, AgentOperation, AgentOperationError, AgentOperationResponse, AgentOutput,
    ApproveRequest, AuditArguments, AuditDecision, DecisionResponse, DenyRequest,
    ListAgentAuditRequest, ListAgentAuditResponse, ListAgentClientsRequest,
    ListAgentClientsResponse, ListGrantsRequest, ListGrantsResponse, ListPendingApprovalsRequest,
    ListPendingApprovalsResponse, OpenAgentSessionRequest, OpenAgentSessionResponse,
    PendingApproval, RevokeGrantRequest, RevokeGrantResponse,
    agent_operation_response::Result as WireResult, agents_server::Agents,
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
    audit: Arc<audit::Store>,
    grants: GrantStore,
    open: requests::Cache<OpenAgentSessionRequest, OpenAgentSessionResponse>,
    decisions: requests::Cache<(String, Outcome), DecisionResponse>,
    revoke: requests::Cache<String, RevokeGrantResponse>,
    grant_admin: tokio::sync::Mutex<()>,
    reads: tokio::sync::Semaphore,
}

impl State {
    pub(super) fn new(paths: marsa_core::paths::MarsaPaths, uid: u32) -> Self {
        Self {
            sessions: sessions::Sessions::default(),
            audit: Arc::new(audit::Store::new(paths.clone(), uid)),
            grants: GrantStore::new(paths, uid),
            open: requests::Cache::new(),
            decisions: requests::Cache::new(),
            revoke: requests::Cache::new(),
            grant_admin: tokio::sync::Mutex::new(()),
            reads: tokio::sync::Semaphore::new(8),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Prepared {
    EngineStatus,
    StopContainer(String),
    DockerRead(reads::Read),
}

impl Prepared {
    fn parse(operation: AgentOperation, args: &str) -> Option<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Empty {}
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Target {
            container_id: String,
        }

        if args.len() > ARGUMENT_LIMIT {
            return None;
        }
        match operation {
            AgentOperation::EngineStatus => {
                serde_json::from_str::<Empty>(args).ok()?;
                Some(Self::EngineStatus)
            }
            AgentOperation::StopContainer => {
                let target: Target = serde_json::from_str(args).ok()?;
                let id = target.container_id;
                (id.len() == 64
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
                .then_some(Self::StopContainer(id))
            }
            _ => reads::Read::parse(operation, args).map(Self::DockerRead),
        }
    }

    const fn operation(&self) -> AgentOperation {
        match self {
            Self::EngineStatus => AgentOperation::EngineStatus,
            Self::StopContainer(_) => AgentOperation::StopContainer,
            Self::DockerRead(read) => read.operation(),
        }
    }

    fn audit_arguments(&self) -> AuditArguments {
        match self {
            Self::EngineStatus => AuditArguments::Empty,
            Self::StopContainer(_) => AuditArguments::ContainerTargetRedacted,
            Self::DockerRead(read) => read.audit_arguments(),
        }
    }

    fn details(&self) -> (String, Vec<AffectedResource>) {
        match self {
            Self::EngineStatus | Self::DockerRead(_) => ("{}".into(), Vec::new()),
            Self::StopContainer(id) => (
                json!({"container_id": id}).to_string(),
                vec![AffectedResource {
                    kind: "container".into(),
                    name: id.clone(),
                    ..Default::default()
                }],
            ),
        }
    }
}

#[derive(Clone)]
struct Execution {
    response: AgentOperationResponse,
    decision: AuditDecision,
}

#[tonic::async_trait]
impl Agents for TypedApi {
    async fn list_audit(
        &self,
        request: Request<ListAgentAuditRequest>,
    ) -> Result<Response<ListAgentAuditResponse>, Status> {
        let filter = self.owner_request(request)?;
        let store = self.inner.agents.audit.clone();
        tokio::task::spawn_blocking(move || store.list(filter))
            .await
            .map_err(|_| self.audit_unavailable())?
            .map(Response::new)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::InvalidInput => Status::invalid_argument(
                    self.agent_text("Invalid audit filter.", "عامل تصفية سجل التدقيق غير صالح."),
                ),
                std::io::ErrorKind::NotFound => Status::aborted(self.agent_text(
                    "The audit cursor has expired. Start a new query.",
                    "انتهت صلاحية مؤشر سجل التدقيق. ابدأ استعلامًا جديدًا.",
                )),
                _ => self.audit_unavailable(),
            })
    }

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

    async fn list_clients(
        &self,
        request: Request<ListAgentClientsRequest>,
    ) -> Result<Response<ListAgentClientsResponse>, Status> {
        self.owner_request(request)?;
        let clients = self.inner.agents.sessions.list()?;
        Ok(Response::new(ListAgentClientsResponse { clients }))
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

    async fn execute_prepared(&self, principal: Arc<Session>, operation: Prepared) -> Execution {
        let mut decision = AuditDecision::Refused;
        let response = self
            .run_reviewed(&principal, operation, &mut decision)
            .await;
        Execution { response, decision }
    }

    async fn run_reviewed(
        &self,
        principal: &Session,
        operation: Prepared,
        decision: &mut AuditDecision,
    ) -> AgentOperationResponse {
        if let Err(result) = self.allow_operation(principal, &operation, decision).await {
            return result;
        }
        match operation {
            Prepared::EngineStatus => {
                let status = self.inner.events.engine_status();
                output(json!({"state":status.state().as_str_name(), "backend":status.backend}))
            }
            Prepared::DockerRead(query) => {
                let Ok(_permit) = self.inner.agents.reads.try_acquire() else {
                    return self.operation_error(
                        "read_busy",
                        "Too many Docker reads are in progress.",
                        "هناك قراءات كثيرة جارية في دوكر.",
                        None,
                    );
                };
                match query.run(self).await {
                    Ok(output) => AgentOperationResponse {
                        result: Some(WireResult::Output(output)),
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => self
                        .operation_error(
                            "not_found",
                            "The Docker resource was not found.",
                            "لم يُعثر على مورد دوكر.",
                            None,
                        ),
                    Err(_) => self.operation_error(
                        "engine_error",
                        "Docker could not complete this read.",
                        "تعذّر على دوكر إكمال هذه القراءة.",
                        None,
                    ),
                }
            }
            Prepared::StopContainer(id) => match engine::stop_container(
                self,
                &id,
                self.permission_still_valid(principal, AgentOperation::StopContainer),
            )
            .await
            {
                Ok(engine::StopResult::Stopped) => {
                    output(json!({"container_id":id,"stopped":true}))
                }
                Ok(engine::StopResult::AlreadyStopped) => {
                    output(json!({"container_id":id,"already_stopped":true}))
                }
                Ok(engine::StopResult::NotFound) => self.operation_error(
                    "not_found",
                    "The container no longer exists.",
                    "لم تعد الحاوية موجودة.",
                    None,
                ),
                Ok(engine::StopResult::Refused) => self.policy_refusal(),
                Err(_) => self.operation_error(
                    "engine_error",
                    "Docker could not confirm the container stopped.",
                    "لم يتمكّن دوكر من تأكيد إيقاف الحاوية.",
                    None,
                ),
            },
        }
    }

    async fn allow_operation(
        &self,
        principal: &Session,
        operation: &Prepared,
        audit_decision: &mut AuditDecision,
    ) -> Result<(), AgentOperationResponse> {
        if !self.scope_is_current(principal).await {
            return Err(self.project_changed());
        }
        let kind = operation.operation();
        let (settings, _) = self.inner.events.settings();
        match decide(kind, Effect::Ordinary, &settings.agents) {
            Decision::Allow => {
                *audit_decision = AuditDecision::Allowed;
                return Ok(());
            }
            Decision::Refuse(_) => return Err(self.policy_refusal()),
            Decision::Approve { tier, .. } => {
                let expires = principal.expires.min(Instant::now() + queue::LIFETIME);
                let (redacted_arguments_json, affected) = operation.details();
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
                    // The initial Docker lifecycle handler cannot establish project
                    // ownership of a container, so the owner approves each target.
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
                    Some(Outcome::Approved) => *audit_decision = AuditDecision::Approved,
                    Some(Outcome::Denied) => {
                        *audit_decision = AuditDecision::Denied;
                        return Err(self.operation_error(
                            "denied",
                            "The owner denied this request.",
                            "رفض المالك هذا الطلب.",
                            Some("owner_denied"),
                        ));
                    }
                    None => {
                        *audit_decision = AuditDecision::Expired;
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

    async fn permission_still_valid(&self, principal: &Session, kind: AgentOperation) -> bool {
        self.scope_is_current(principal).await
            && principal.expires > Instant::now()
            && !*self.inner.closing.borrow()
            && !matches!(
                decide(
                    kind,
                    Effect::Ordinary,
                    &self.inner.events.settings().0.agents
                ),
                Decision::Refuse(_)
            )
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
        AgentOperationResponse {
            result: Some(WireResult::Error(AgentOperationError {
                code: code.into(),
                message: self.agent_text(en, ar).into(),
                denial_reason: reason.map(str::to_owned),
            })),
        }
    }

    fn audit_unavailable(&self) -> Status {
        Status::unavailable(self.agent_text(
            "The agent audit log is unavailable.",
            "سجل تدقيق الوكيل غير متاح.",
        ))
    }

    fn agent_text<'a>(&self, en: &'a str, ar: &'a str) -> &'a str {
        let language = self.inner.events.settings().0.ui.language;
        let arabic = language == Language::Arabic
            || (language == Language::System
                && ["LC_ALL", "LC_MESSAGES", "LANG"]
                    .into_iter()
                    .filter_map(|key| std::env::var(key).ok())
                    .find(|value| !value.is_empty())
                    .is_some_and(|value| value.split(['_', '-', '.', '@']).next() == Some("ar")));
        if arabic { ar } else { en }
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
