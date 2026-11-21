//! Serve the implemented Meta RPCs over the owner's Unix socket.

use std::{future::Future, time::Duration};

use marsa_proto::host::{
    API_MAJOR, API_MINOR,
    v1::{
        CapabilitiesRequest, CapabilitiesResponse, VersionRequest, VersionResponse,
        meta_server::{Meta, MetaServer},
    },
};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{
    Request, Response, Status,
    transport::{self, Server, server::UdsConnectInfo},
};

const MAX_META_BYTES: usize = 4096;

pub(super) async fn serve(
    listener: UnixListener,
    uid: u32,
    stop: impl Future<Output = ()>,
) -> Result<(), transport::Error> {
    let meta = MetaServer::new(MetaApi { uid })
        .max_decoding_message_size(MAX_META_BYTES)
        .max_encoding_message_size(MAX_META_BYTES);
    Server::builder()
        .timeout(Duration::from_secs(2))
        .add_service(meta)
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), stop)
        .await
}

struct MetaApi {
    uid: u32,
}

impl MetaApi {
    fn owns_peer<T>(&self, request: &Request<T>) -> bool {
        let uid = request
            .extensions()
            .get::<UdsConnectInfo>()
            .and_then(|info| info.peer_cred.map(|credentials| credentials.uid()));
        uid == Some(self.uid)
    }
}

#[tonic::async_trait]
impl Meta for MetaApi {
    async fn version(
        &self,
        request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        if !self.owns_peer(&request) {
            return Err(Status::permission_denied(
                "the caller does not own this daemon socket",
            ));
        }
        Ok(Response::new(VersionResponse {
            api_major: API_MAJOR,
            api_minor: API_MINOR,
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            components: Vec::new(),
        }))
    }

    async fn capabilities(
        &self,
        request: Request<CapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        if !self.owns_peer(&request) {
            return Err(Status::permission_denied(
                "the caller does not own this daemon socket",
            ));
        }
        Ok(Response::new(CapabilitiesResponse {
            backends: Vec::new(),
        }))
    }
}
