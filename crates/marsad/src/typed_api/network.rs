//! Keep the configured attachment separate from observations of the running VM.

use marsa_core::settings::NetworkMode as ConfigMode;
use marsa_proto::host::v1::{
    ActivateProxyRequest, ListForwardedPortsRequest, ListForwardedPortsResponse, NetworkMode,
    NetworkStatus, NetworkStatusRequest, PrepareProxyActivationRequest, ProxyActivationPreview,
    ProxyActivationResult, SetNetworkModeRequest, network_server::Network,
};
use tonic::{Request, Response, Status};

use crate::supervisor::NetworkObservation;

use super::{TypedApi, events};

#[tonic::async_trait]
impl Network for TypedApi {
    async fn status(
        &self,
        request: Request<NetworkStatusRequest>,
    ) -> Result<Response<NetworkStatus>, Status> {
        self.authorize(request)?;
        let configured = self.inner.events.configured_network_mode();
        let active = self.observe_network(configured).await?;
        Ok(Response::new(network_status(configured, active)))
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
        let request = self.authorize(request)?;
        let mode = match NetworkMode::try_from(request.mode) {
            Ok(NetworkMode::Auto) => ConfigMode::Auto,
            Ok(NetworkMode::Nat) => ConfigMode::Nat,
            Ok(NetworkMode::Userspace) => ConfigMode::Userspace,
            _ => {
                return Err(Status::invalid_argument(
                    "Unsupported network mode. Choose auto, nat, or userspace. | وضع الشبكة غير مدعوم. اختر auto أو nat أو userspace.",
                ));
            }
        };
        let api = self.clone();
        let status = self
            .inner
            .network_mode_requests
            .run(&request.request_id, mode, || async move {
                api.edit_settings(None, move |settings, _| {
                    settings.network.mode = mode;
                    Ok(())
                })
                .await?;
                // The edit has committed even if the VM observation is unavailable.
                let active = api.observe_network(mode).await.unwrap_or_default();
                Ok(network_status(mode, active))
            })
            .await?;
        Ok(Response::new(status))
    }

    async fn list_forwarded_ports(
        &self,
        request: Request<ListForwardedPortsRequest>,
    ) -> Result<Response<ListForwardedPortsResponse>, Status> {
        self.authorize(request)?;
        Ok(Response::new(self.inner.events.forwarded_ports()))
    }
}

impl TypedApi {
    async fn observe_network(&self, desired: ConfigMode) -> Result<NetworkObservation, Status> {
        match &self.supervisor {
            Some(supervisor) => supervisor
                .observe_network(desired)
                .await
                .map_err(|_| Status::unavailable("guest supervisor is unavailable")),
            None => Ok(NetworkObservation {
                restart_pending: Some(false),
                ..Default::default()
            }),
        }
    }
}

fn network_status(configured: ConfigMode, observed: NetworkObservation) -> NetworkStatus {
    let Some((boot, facts)) = observed.attachment else {
        return NetworkStatus {
            configured_mode: events::mode(configured).into(),
            restart_pending: observed.restart_pending,
            ..Default::default()
        };
    };
    let mut status = events::network_status(configured, boot);
    status.restart_pending = observed.restart_pending;
    if let Some(facts) = facts {
        status.guest_address = Some(facts.address.to_string());
        status.gateway_address = facts.gateway.map(|address| address.to_string());
    }
    status
}
