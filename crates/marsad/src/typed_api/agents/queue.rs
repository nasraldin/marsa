//! Keep pending approval state and decisions under the daemon event cursor lock.

use std::{collections::VecDeque, time::Duration};

use marsa_proto::host::v1::{ListPendingApprovalsResponse, PendingApproval};
use tokio::{sync::oneshot, time::Instant};
use tonic::Status;

pub(super) const LIFETIME: Duration = Duration::from_secs(120);
const LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::typed_api) enum Outcome {
    Approved,
    Denied,
}

#[derive(Default)]
pub(in crate::typed_api) struct Queue(VecDeque<Entry>);

struct Entry {
    approval: PendingApproval,
    expires: Instant,
    response: oneshot::Sender<Outcome>,
}

impl Queue {
    pub(in crate::typed_api) fn add(
        &mut self,
        mut approval: PendingApproval,
        expires: Instant,
        now: Instant,
    ) -> Result<(String, oneshot::Receiver<Outcome>), Status> {
        self.0.retain(|entry| entry.expires > now);
        if self.0.len() >= LIMIT {
            return Err(Status::resource_exhausted(
                "the agent approval queue is full",
            ));
        }
        let mut random = [0; 16];
        getrandom::fill(&mut random)
            .map_err(|_| Status::internal("cannot create approval request"))?;
        let id = format!("{:032x}", u128::from_be_bytes(random));
        if self.0.iter().any(|entry| entry.approval.id == id) {
            return Err(Status::internal("cannot create approval request"));
        }
        approval.id = id.clone();
        let (response, wait) = oneshot::channel();
        self.0.push_back(Entry {
            approval,
            expires,
            response,
        });
        Ok((id, wait))
    }

    pub(in crate::typed_api) fn list(&self, now: Instant) -> ListPendingApprovalsResponse {
        ListPendingApprovalsResponse {
            approvals: self
                .0
                .iter()
                .filter(|entry| now < entry.expires)
                .map(|entry| entry.approval.clone())
                .collect(),
        }
    }

    pub(in crate::typed_api) fn remove(&mut self, id: &str) -> bool {
        let Some(index) = self.0.iter().position(|entry| entry.approval.id == id) else {
            return false;
        };
        self.0.remove(index).is_some()
    }

    pub(in crate::typed_api) fn resolve(
        &mut self,
        id: &str,
        outcome: Outcome,
        now: Instant,
    ) -> Result<(), Status> {
        let Some(index) = self
            .0
            .iter()
            .position(|entry| entry.approval.id == id && now < entry.expires)
        else {
            return Err(Status::not_found("unknown or expired agent approval"));
        };
        let entry = self
            .0
            .remove(index)
            .ok_or_else(|| Status::not_found("unknown agent approval"))?;
        entry
            .response
            .send(outcome)
            .map_err(|_| Status::not_found("the agent call ended"))
    }
}
