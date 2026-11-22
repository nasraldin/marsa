//! Expose the chosen guest attachment and its observed network facts.

use marsa_proto::host::v1::{
    ListForwardedPortsRequest, ListForwardedPortsResponse, NetworkStatus, NetworkStatusRequest,
    SetNetworkModeRequest, network_server::Network,
};
use tonic::{Request, Response, Status};

use super::{TypedApi, events};

#[tonic::async_trait]
impl Network for TypedApi {
    async fn status(
        &self,
        request: Request<NetworkStatusRequest>,
    ) -> Result<Response<NetworkStatus>, Status> {
        self.authorize(request)?;
        let active = match &self.supervisor {
            Some(supervisor) => supervisor
                .active_network()
                .await
                .map_err(|_| Status::unavailable("guest supervisor is unavailable"))?,
            None => None,
        };
        let configured = self.inner.events.configured_network_mode();
        let status = match active {
            Some((boot, facts)) => {
                let mut status = events::network_status(configured, boot);
                if let Some(facts) = facts {
                    status.guest_address = Some(facts.address.to_string());
                    status.gateway_address = facts.gateway.map(|address| address.to_string());
                }
                status
            }
            None => NetworkStatus {
                configured_mode: events::mode(configured).into(),
                ..Default::default()
            },
        };
        Ok(Response::new(status))
    }

    async fn set_mode(
        &self,
        request: Request<SetNetworkModeRequest>,
    ) -> Result<Response<NetworkStatus>, Status> {
        self.authorize(request)?;
        Err(Status::unimplemented(
            "setting the network mode is unavailable",
        ))
    }

    async fn list_forwarded_ports(
        &self,
        request: Request<ListForwardedPortsRequest>,
    ) -> Result<Response<ListForwardedPortsResponse>, Status> {
        self.authorize(request)?;
        Err(Status::unimplemented(
            "listing forwarded ports is unavailable",
        ))
    }
}
