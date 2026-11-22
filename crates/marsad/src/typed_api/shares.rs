//! Manage explicitly configured shares through the same atomic settings revision.

use std::path::Path;

use marsa_proto::host::v1::{
    AddShareRequest, ListSharesRequest, ListSharesResponse, RemoveShareRequest,
    RemoveShareResponse, Share, shares_server::Shares,
};
use marsa_share::{Allowlist, ShareError};
use tonic::{Request, Response, Status};

use super::TypedApi;

#[tonic::async_trait]
impl Shares for TypedApi {
    async fn list(
        &self,
        request: Request<ListSharesRequest>,
    ) -> Result<Response<ListSharesResponse>, Status> {
        self.authorize(request)?;
        let (settings, _) = self.inner.events.settings();
        let shares = settings
            .shares
            .iter()
            .map(|path| wire_share(path))
            .collect::<Result<_, _>>()?;
        Ok(Response::new(ListSharesResponse { shares }))
    }

    async fn add(&self, request: Request<AddShareRequest>) -> Result<Response<Share>, Status> {
        let mut request = self.authorize(request)?;
        let id = std::mem::take(&mut request.request_id);
        let api = self.clone();
        let input = request.clone();
        let share = self
            .inner
            .add_share_requests
            .run(&id, input, || async move {
                if request.approval_id.is_some() || !request.persist {
                    return Err(Status::unimplemented(
                        "bind prompts and session shares are unavailable",
                    ));
                }
                if request.read_only {
                    return Err(Status::unimplemented(
                        "read-only share grants are unavailable",
                    ));
                }
                let (_, share) = api
                    .edit_settings(None, move |settings, policy| {
                        let mut allowlist = Allowlist::from_paths(policy.clone(), &settings.shares)
                            .map_err(policy_status)?;
                        let path = allowlist
                            .add(Path::new(&request.host_path))
                            .map_err(policy_status)?;
                        let share = wire_share(path)?;
                        settings.shares = allowlist.roots().to_vec();
                        Ok(share)
                    })
                    .await?;
                Ok(share)
            })
            .await?;
        Ok(Response::new(share))
    }

    async fn remove(
        &self,
        request: Request<RemoveShareRequest>,
    ) -> Result<Response<RemoveShareResponse>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let result = self
            .inner
            .remove_share_requests
            .run(
                &request.request_id,
                request.share_id.clone(),
                || async move {
                    let (_, ()) = api
                        .edit_settings(None, move |settings, _| {
                            let path = Path::new(&request.share_id);
                            let index = settings
                                .shares
                                .iter()
                                .position(|entry| entry == path)
                                .ok_or_else(|| Status::not_found("unknown share ID"))?;
                            settings.shares.remove(index);
                            Ok(())
                        })
                        .await?;
                    Ok(RemoveShareResponse {
                        requires_restart: true,
                    })
                },
            )
            .await?;
        Ok(Response::new(result))
    }
}

fn wire_share(path: &Path) -> Result<Share, Status> {
    let host_path = path
        .to_str()
        .ok_or_else(|| Status::internal("configured share path is not UTF-8"))?
        .to_owned();
    Ok(Share {
        id: host_path.clone(),
        host_path,
        read_only: false,
        requires_restart: true,
    })
}

pub(super) fn policy_status(error: ShareError) -> Status {
    let code = match error {
        ShareError::InvalidPath | ShareError::NotDirectory | ShareError::UnsupportedSource => {
            tonic::Code::InvalidArgument
        }
        ShareError::Resolve(_) | ShareError::Protection(_) => tonic::Code::FailedPrecondition,
        ShareError::Protected | ShareError::NotAllowed => tonic::Code::PermissionDenied,
    };
    Status::new(code, error.to_string())
}
