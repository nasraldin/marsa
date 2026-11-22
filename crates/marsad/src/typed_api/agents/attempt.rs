//! Record each decoded RPC independently of engine execution deduplication.

use marsa_proto::host::v1::{
    AgentOperation, AgentOperationRequest, AgentOperationResponse, AuditDecision, AuditResult,
    agent_operation_response::Result as WireResult, agent_ops_server::AgentOps,
};
use tonic::{Request, Response, Status};

use super::{Prepared, TypedApi, audit, reads};

#[tonic::async_trait]
impl AgentOps for TypedApi {
    async fn execute(
        &self,
        request: Request<AgentOperationRequest>,
    ) -> Result<Response<AgentOperationResponse>, Status> {
        let mut record = audit::Record::new(request.get_ref().operation())
            .map_err(|_| self.audit_unavailable())?;
        let api = self.clone();
        // Keep the audit attempt alive if the client disconnects while reviewed
        // work or its durable append is still in progress.
        tokio::spawn(async move {
            let result = api.execute_attempt(request, &mut record).await;
            let store = api.inner.agents.audit.clone();
            match tokio::task::spawn_blocking(move || store.append(record)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(kind = ?error.kind(), "could not append agent audit");
                    if result.is_ok() {
                        return Err(api.audit_unavailable());
                    }
                }
                Err(_) => {
                    tracing::warn!("agent audit worker stopped");
                    if result.is_ok() {
                        return Err(api.audit_unavailable());
                    }
                }
            }
            result.map(Response::new)
        })
        .await
        .map_err(|_| Status::internal("agent execution worker stopped"))?
    }
}

impl TypedApi {
    async fn execute_attempt(
        &self,
        request: Request<AgentOperationRequest>,
        record: &mut audit::Record,
    ) -> Result<AgentOperationResponse, Status> {
        let session = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .and_then(|token| self.inner.agents.sessions.authenticate(token));
        let request = self.authorize(request).inspect_err(|status| {
            if status.code() == tonic::Code::PermissionDenied {
                record.complete(AuditDecision::Refused, AuditResult::Unauthenticated);
            }
        })?;
        let Some(session) = session else {
            record.complete(AuditDecision::Refused, AuditResult::Unauthenticated);
            return Err(Status::unauthenticated("valid agent session required"));
        };
        record.verified_client(&session.client_id);
        if request.client_id != session.client_id || request.project_path != session.scope.project()
        {
            return Err(Status::permission_denied(
                "request is outside the registered agent scope",
            ));
        }
        let operation = request.operation();
        if operation != AgentOperation::EngineStatus
            && operation != AgentOperation::StopContainer
            && !reads::Read::supports(operation)
        {
            return Ok(self.operation_error(
                "unsupported_operation",
                "This agent operation is unavailable.",
                "عملية الوكيل هذه غير متاحة.",
                None,
            ));
        }
        let Some(operation) = Prepared::parse(operation, &request.arguments_json) else {
            return Ok(self.operation_error(
                "invalid_arguments",
                "Invalid agent operation arguments.",
                "وسائط عملية الوكيل غير صالحة.",
                None,
            ));
        };
        record.arguments(operation.audit_arguments());
        let store = self.inner.agents.audit.clone();
        tokio::task::spawn_blocking(move || store.check())
            .await
            .map_err(|_| self.audit_unavailable())?
            .map_err(|_| self.audit_unavailable())?;
        let api = self.clone();
        let principal = session.clone();
        let result = session
            .requests
            .run(&request.request_id, operation.clone(), || async move {
                Ok(api.execute_prepared(principal, operation).await)
            })
            .await?;
        let outcome = match &result.response.result {
            Some(WireResult::Output(_)) => AuditResult::Succeeded,
            Some(WireResult::Error(error)) => match error.code.as_str() {
                "denied" => AuditResult::Denied,
                "approval_timed_out" => AuditResult::TimedOut,
                "engine_error" | "not_found" => AuditResult::Failed,
                _ => AuditResult::Rejected,
            },
            None => AuditResult::Failed,
        };
        record.complete(result.decision, outcome);
        Ok(result.response)
    }
}
