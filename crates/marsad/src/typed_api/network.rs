//! Expose the chosen guest attachment and its observed network facts.

use marsa_proto::host::v1::{
    ActivateProxyRequest, ListForwardedPortsRequest, ListForwardedPortsResponse, NetworkStatus,
    NetworkStatusRequest, PrepareProxyActivationRequest, ProxyActivationPreview,
    ProxyActivationResult, SetNetworkModeRequest, network_server::Network,
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

    async fn prepare_proxy_activation(
        &self,
        request: Request<PrepareProxyActivationRequest>,
    ) -> Result<Response<ProxyActivationPreview>, Status> {
        let request = self.authorize(request)?;
        self.preview_proxy(request.expected_settings_revision)
            .await
            .map(Response::new)
    }

    async fn activate_proxy(
        &self,
        request: Request<ActivateProxyRequest>,
    ) -> Result<Response<ProxyActivationResult>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let request_id = request.request_id.clone();
        self.inner
            .proxy_requests
            .run(&request_id, request.clone(), move || async move {
                api.apply_proxy(request).await
            })
            .await
            .map(Response::new)
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
