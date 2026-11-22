//! Serve one bounded authorization stream opened to the running VM.

use std::{sync::Arc, time::Duration};

use marsa_proto::guest::{
    BIND_AUTHORIZATION_PORT, BIND_AUTHORIZATION_VERSION, bind_authorization_frame_length,
    decode_bind_authorization_request, encode_bind_authorization_response,
    v1::{BindAuthorizationDecision, BindAuthorizationResponse},
};
use marsa_vm::{Vm, VsockStream};
use tokio::{io::AsyncReadExt, task::JoinHandle, time::timeout};

use super::{BindPolicy, GuestControlError, wire};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) async fn connect(
    vm: &dyn Vm,
    policy: Arc<dyn BindPolicy>,
) -> Result<JoinHandle<()>, GuestControlError> {
    let stream = timeout(CONNECT_TIMEOUT, wire::open(vm, BIND_AUTHORIZATION_PORT))
        .await
        .map_err(|_| GuestControlError::TimedOut("connecting to guest bind authorization"))??;
    Ok(tokio::spawn(async move {
        let _ = serve(stream, policy).await;
    }))
}

async fn serve(
    mut stream: VsockStream,
    policy: Arc<dyn BindPolicy>,
) -> Result<(), GuestControlError> {
    let mut expected = 1_u64;
    loop {
        let mut header = [0_u8; 4];
        if stream.read(&mut header[..1]).await? == 0 {
            return Ok(());
        }
        let request = timeout(FRAME_TIMEOUT, async {
            stream.read_exact(&mut header[1..]).await?;
            let mut payload = vec![0; bind_authorization_frame_length(header)?];
            stream.read_exact(&mut payload).await?;
            Ok::<_, GuestControlError>(decode_bind_authorization_request(&payload)?)
        })
        .await
        .map_err(|_| GuestControlError::TimedOut("reading a bind authorization query"))??;
        if request.request_id != expected {
            return Err(GuestControlError::Sequence("unexpected bind request ID"));
        }
        let denial = policy.authorize(request.sources).await;
        let decision = if denial.is_some() {
            BindAuthorizationDecision::Deny
        } else {
            BindAuthorizationDecision::Allow
        };
        wire::write(
            &mut stream,
            &encode_bind_authorization_response(&BindAuthorizationResponse {
                protocol: Some(BIND_AUTHORIZATION_VERSION),
                request_id: expected,
                decision: decision.into(),
                message: denial.unwrap_or_default().into(),
            })?,
        )
        .await?;
        expected = expected
            .checked_add(1)
            .ok_or(GuestControlError::Sequence("bind request ID exhausted"))?;
    }
}
