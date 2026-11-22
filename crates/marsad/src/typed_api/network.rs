//! Keep the configured attachment separate from observations of the running VM.

use marsa_core::settings::NetworkMode as ConfigMode;
use marsa_proto::host::v1::{
    ListForwardedPortsRequest, ListForwardedPortsResponse, NetworkMode, NetworkStatus,
    NetworkStatusRequest, SetNetworkModeRequest, network_server::Network,
};
use marsa_vm::NetworkInfo;
use tonic::{Request, Response, Status};

use crate::supervisor::BootNetwork;

use super::{TypedApi, events};

type ActiveAttachment = Option<(BootNetwork, Option<NetworkInfo>)>;

#[tonic::async_trait]
impl Network for TypedApi {
    async fn status(
        &self,
        request: Request<NetworkStatusRequest>,
    ) -> Result<Response<NetworkStatus>, Status> {
        self.authorize(request)?;
        let active = self.observe_network().await?;
        let configured = self.inner.events.configured_network_mode();
        Ok(Response::new(network_status(configured, active)))
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
                let active = api.observe_network().await.unwrap_or_default();
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
        Err(Status::unimplemented(
            "listing forwarded ports is unavailable",
        ))
    }
}

impl TypedApi {
    async fn observe_network(&self) -> Result<ActiveAttachment, Status> {
        match &self.supervisor {
            Some(supervisor) => supervisor
                .active_network()
                .await
                .map_err(|_| Status::unavailable("guest supervisor is unavailable")),
            None => Ok(None),
        }
    }
}

fn network_status(configured: ConfigMode, active: ActiveAttachment) -> NetworkStatus {
    let Some((boot, facts)) = active else {
        return NetworkStatus {
            configured_mode: events::mode(configured).into(),
            ..Default::default()
        };
    };
    let mut status = events::network_status(configured, boot);
    if let Some(facts) = facts {
        status.guest_address = Some(facts.address.to_string());
        status.gateway_address = facts.gateway.map(|address| address.to_string());
    }
    status
}
