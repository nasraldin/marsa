//! Report daemon and protocol versions over the typed API.

use marsa_proto::host::{
    API_MAJOR, API_MINOR,
    v1::{
        CapabilitiesRequest, CapabilitiesResponse, VersionRequest, VersionResponse, VmCapabilities,
        meta_server::Meta,
    },
};
use tonic::{Request, Response, Status};

use super::TypedApi;

#[tonic::async_trait]
impl Meta for TypedApi {
    async fn version(
        &self,
        request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        self.authorize(request)?;
        Ok(Response::new(VersionResponse {
            api_major: API_MAJOR,
            api_minor: API_MINOR,
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            components: self.components.to_vec(),
        }))
    }

    async fn capabilities(
        &self,
        request: Request<CapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        self.authorize(request)?;
        Ok(Response::new(CapabilitiesResponse {
            backends: self
                .capabilities
                .iter()
                .map(|c| VmCapabilities {
                    backend: c.name.into(),
                    memory_balloon: c.memory_balloon,
                    pause_resume: c.pause_resume,
                    dynamic_shares: c.dynamic_shares,
                    userspace_network: c.userspace_network,
                })
                .collect(),
        }))
    }
}
