//! Read bounded guest frames and independently verify the Docker API bridge.

use std::{io, time::Duration};

use marsa_proto::guest::{
    DOCKER_API_PORT, decode_guest, encode_host, frame_length,
    v1::{GuestEnvelope, HostEnvelope, host_envelope::Body},
};
use marsa_vm::{Vm, VmError, VsockStream};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{sleep, timeout},
};

use super::{CONNECT_RETRY, GuestControlError};

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const PING_TIMEOUT: Duration = Duration::from_secs(1);
const PING_REQUEST: &[u8] = b"GET /_ping HTTP/1.0\r\nHost: docker\r\nConnection: close\r\n\r\n";

pub(super) async fn open(vm: &dyn Vm, port: u32) -> Result<VsockStream, GuestControlError> {
    loop {
        match vm.connect_vsock(port).await {
            Ok(stream) => return Ok(stream),
            Err(VmError::GuestPortUnavailable { .. }) => sleep(CONNECT_RETRY).await,
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) fn encode(id: u64, body: Body) -> Result<Vec<u8>, GuestControlError> {
    Ok(encode_host(&HostEnvelope {
        request_id: id,
        body: Some(body),
    })?)
}

pub(super) async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    frame: &[u8],
) -> Result<(), GuestControlError> {
    timeout(WRITE_TIMEOUT, writer.write_all(frame))
        .await
        .map_err(|_| GuestControlError::TimedOut("writing a frame"))??;
    Ok(())
}

pub(super) async fn read(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<GuestEnvelope, GuestControlError> {
    // Idle streams may be silent. Once a peer starts a frame, both the rest of
    // the header and the payload must finish within one deadline.
    let mut header = [0; 4];
    reader.read_exact(&mut header[..1]).await?;
    timeout(FRAME_TIMEOUT, async {
        reader.read_exact(&mut header[1..]).await?;
        let length = frame_length(header)?;
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload).await?;
        Ok(decode_guest(&payload)?)
    })
    .await
    .map_err(|_| GuestControlError::TimedOut("reading a partial frame"))?
}

pub(super) async fn docker_ping(vm: &dyn Vm) -> Result<bool, GuestControlError> {
    // The HTTP probe shares the boot deadline but each individual attempt
    // bounds the time the guest may hold a bridge connection open.
    match timeout(PING_TIMEOUT, async {
        let mut stream = open(vm, DOCKER_API_PORT).await?;
        stream.write_all(PING_REQUEST).await?;
        status(&mut stream).await.map_err(GuestControlError::from)
    })
    .await
    {
        Ok(Ok(success)) => Ok(success),
        Ok(Err(GuestControlError::Io(_))) | Err(_) => Ok(false),
        Ok(Err(error)) => Err(error),
    }
}

async fn status(stream: &mut VsockStream) -> io::Result<bool> {
    let mut bytes = [0; 128];
    let mut filled = 0;
    while filled < bytes.len() {
        let count = stream.read(&mut bytes[filled..]).await?;
        if count == 0 {
            break;
        }
        let next = filled + count;
        if let Some(newline) = bytes[filled..next].iter().position(|byte| *byte == b'\n') {
            let line = &bytes[..filled + newline];
            return Ok(line.ends_with(b"\r")
                && (line.starts_with(b"HTTP/1.0 200 ") || line.starts_with(b"HTTP/1.1 200 ")));
        }
        filled = next;
    }
    Ok(false)
}
