//! Drive the supervisor with explicit and deduplicated lifecycle requests.

use marsa_proto::{
    guest::v1::{EngineHealth, EngineHealthState as GuestHealth},
    host::v1::{
        EngineActionRequest, EngineHealthState, EngineState, EngineStatus, EngineStatusRequest,
        ResetRequest, ResetResponse, engine_server::Engine,
    },
};
use tonic::{Request, Response, Status};

use crate::supervisor::{BootStage, State, SupervisorError, Transition};

use super::TypedApi;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Action {
    Start,
    Stop,
    Restart,
}

impl TypedApi {
    /// Publish a health value from the active, validated guest session.
    pub fn publish_guest_health(&self, health: &EngineHealth) {
        let health = match health.state() {
            GuestHealth::Unspecified => return,
            GuestHealth::Starting => EngineHealthState::Starting,
            GuestHealth::Ready => EngineHealthState::Ready,
            GuestHealth::Unhealthy => EngineHealthState::Unhealthy,
            GuestHealth::Restarting => EngineHealthState::Restarting,
        };
        self.inner.events.health(health);
    }

    async fn action(
        &self,
        request: Request<EngineActionRequest>,
        action: Action,
    ) -> Result<Response<EngineStatus>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let status = self
            .inner
            .engine_requests
            .run(&request.request_id, action, || async move {
                let supervisor = api
                    .supervisor
                    .as_ref()
                    .ok_or_else(|| Status::unavailable("no VM runtime is attached"))?;
                match action {
                    Action::Start => supervisor.ensure_ready().await,
                    Action::Stop => supervisor.stop().await,
                    Action::Restart => supervisor.restart().await,
                }
                .map_err(supervisor_status)?;
                Ok(api.inner.events.engine_status())
            })
            .await?;
        Ok(Response::new(status))
    }
}

#[tonic::async_trait]
impl Engine for TypedApi {
    async fn status(
        &self,
        request: Request<EngineStatusRequest>,
    ) -> Result<Response<EngineStatus>, Status> {
        self.authorize(request)?;
        Ok(Response::new(self.inner.events.engine_status()))
    }

    async fn start(
        &self,
        request: Request<EngineActionRequest>,
    ) -> Result<Response<EngineStatus>, Status> {
        self.action(request, Action::Start).await
    }

    async fn stop(
        &self,
        request: Request<EngineActionRequest>,
    ) -> Result<Response<EngineStatus>, Status> {
        self.action(request, Action::Stop).await
    }

    async fn restart(
        &self,
        request: Request<EngineActionRequest>,
    ) -> Result<Response<EngineStatus>, Status> {
        self.action(request, Action::Restart).await
    }

    async fn reset(
        &self,
        request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        self.authorize(request)?;
        Err(Status::unimplemented("engine reset is not available"))
    }
}

pub(super) fn from_transition(backend: &str, transition: &Transition, health: i32) -> EngineStatus {
    EngineStatus {
        state: wire_state(transition.current).into(),
        backend: backend.into(),
        boot_stage: transition
            .failure
            .as_ref()
            .map(|failure| stage_name(failure.stage).into()),
        reason: transition
            .failure
            .as_ref()
            .map(|failure| bounded(&failure.detail)),
        console_tail: transition
            .failure
            .as_ref()
            .map(|failure| bounded(&failure.console_tail)),
        guest_health: health,
    }
}

pub(super) fn wire_state(state: State) -> EngineState {
    match state {
        State::Stopped => EngineState::Stopped,
        State::Starting => EngineState::Starting,
        State::Booting => EngineState::Booting,
        State::Ready => EngineState::Ready,
        State::Idle => EngineState::Idle,
        State::Paused => EngineState::Paused,
        State::Stopping => EngineState::Stopping,
        State::Failed => EngineState::Failed,
    }
}

pub(super) fn stage_name(stage: BootStage) -> &'static str {
    match stage {
        BootStage::Create => "create",
        BootStage::Start => "start",
        BootStage::Handshake => "handshake",
        BootStage::Network => "network",
        BootStage::Engine => "engine",
        BootStage::Running => "running",
        BootStage::Resume => "resume",
        BootStage::Shutdown => "shutdown",
    }
}

fn bounded(text: &str) -> String {
    text[..text.floor_char_boundary(text.len().min(8192))].to_owned()
}

fn supervisor_status(error: SupervisorError) -> Status {
    match error {
        SupervisorError::Busy => Status::resource_exhausted(error.to_string()),
        SupervisorError::InvalidState { .. } => Status::failed_precondition(error.to_string()),
        SupervisorError::Boot(ref failure) if failure.timed_out => {
            Status::deadline_exceeded(error.to_string())
        }
        _ => Status::unavailable(error.to_string()),
    }
}
