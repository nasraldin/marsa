//! Relay an owner-authorized transfer only to the already-running built-in guest.

use std::{io, time::Duration};

use marsa_proto::{
    guest::{
        VOLUME_READ_LIMIT, decode_volume_reply, encode_volume_request,
        v1::{
            VolumeReadReply, VolumeReadRequest, volume_read_reply::Body as Reply,
            volume_read_request::Body as Input,
        },
        volume_frame_length,
    },
    host::v1::volume_files_server::VolumeFiles,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    time::timeout,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::supervisor::DockerStream;

use super::TypedApi;

const IDLE: Duration = Duration::from_secs(60);
const REVIEW_INPUT: Duration = Duration::from_secs(180);
const TRANSFER: Duration = Duration::from_secs(30 * 60);

type Output = mpsc::Sender<Result<VolumeReadReply, Status>>;

#[tonic::async_trait]
impl VolumeFiles for TypedApi {
    type ReadStream = ReceiverStream<Result<VolumeReadReply, Status>>;

    async fn read(
        &self,
        request: Request<Streaming<VolumeReadRequest>>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        let mut inbound = self.authorize(request)?;
        let first = next(&mut inbound, IDLE).await?;
        let probe = match &first.body {
            Some(Input::Probe(true)) => true,
            Some(Input::Select(_)) => false,
            _ => return Err(Status::invalid_argument("volume selection required")),
        };
        let header = encode_volume_request(&first)
            .map_err(|_| Status::invalid_argument("invalid volume selection"))?;
        let supervisor = self
            .supervisor
            .as_ref()
            .ok_or_else(|| Status::unimplemented("volume provider unavailable"))?;
        let mut guest =
            supervisor
                .connect_running_volume_files()
                .await
                .map_err(|error| match error.kind() {
                    io::ErrorKind::NotConnected => {
                        Status::failed_precondition("volume reads require a running VM")
                    }
                    io::ErrorKind::WouldBlock => Status::unavailable("volume provider is busy"),
                    _ => Status::unimplemented("volume provider unavailable"),
                })?;
        timeout(IDLE, guest.write_all(&header))
            .await
            .map_err(|_| Status::deadline_exceeded("volume provider timed out"))?
            .map_err(|_| Status::unimplemented("volume provider unavailable"))?;
        let (out, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let error = match timeout(TRANSFER, relay(&mut guest, &mut inbound, &out, probe)).await
            {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error,
                Err(_) => Status::deadline_exceeded("volume transfer timed out"),
            };
            let _ = out.send(Err(error)).await;
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

async fn relay(
    guest: &mut DockerStream,
    inbound: &mut Streaming<VolumeReadRequest>,
    out: &Output,
    probe: bool,
) -> Result<(), Status> {
    let first = tokio::select! {
        () = out.closed() => return Ok(()),
        first = read_guest(guest) => first?,
    };
    match first.body {
        Some(Reply::Failure(_)) => return forward(out, first).await,
        Some(Reply::Capable(true)) if probe => return forward(out, first).await,
        Some(Reply::Selected(true)) if !probe => forward(out, first).await?,
        _ => return Err(malformed()),
    }

    let prepared = tokio::select! {
        () = out.closed() => return Ok(()),
        prepared = next(inbound, REVIEW_INPUT) => prepared?,
    };
    if !matches!(prepared.body, Some(Input::Prepare(_))) {
        return Err(Status::invalid_argument("volume path required"));
    }
    write_guest(guest, &prepared).await?;
    let format = tokio::select! {
        () = out.closed() => return Ok(()),
        format = read_guest(guest) => format?,
    };
    match format.body {
        Some(Reply::Failure(_)) => return forward(out, format).await,
        Some(Reply::Format(_)) => forward(out, format).await?,
        _ => return Err(malformed()),
    }

    let start = tokio::select! {
        () = out.closed() => return Ok(()),
        start = next(inbound, REVIEW_INPUT) => start?,
    };
    if start.body != Some(Input::Start(true)) {
        return Err(Status::invalid_argument(
            "volume destination confirmation required",
        ));
    }
    write_guest(guest, &start).await?;

    let mut received = 0;
    loop {
        let reply = tokio::select! {
            () = out.closed() => return Ok(()),
            next = read_guest(guest) => next?,
        };
        match &reply.body {
            Some(Reply::Data(bytes)) => {
                received += bytes.len() as u64;
                if received > VOLUME_READ_LIMIT {
                    return Err(Status::resource_exhausted(
                        "volume transfer exceeds its limit",
                    ));
                }
                forward(out, reply).await?;
            }
            Some(Reply::Complete(true)) => {
                let mut extra = [0];
                let end = tokio::select! {
                    () = out.closed() => return Ok(()),
                    end = timeout(IDLE, guest.read(&mut extra)) => {
                        end.map_err(|_| timed_out())?.map_err(|_| malformed())?
                    }
                };
                return if end == 0 {
                    forward(out, reply).await
                } else {
                    Err(malformed())
                };
            }
            Some(Reply::Failure(_)) => return forward(out, reply).await,
            _ => return Err(malformed()),
        }
    }
}

async fn write_guest(guest: &mut DockerStream, request: &VolumeReadRequest) -> Result<(), Status> {
    let frame = encode_volume_request(request)
        .map_err(|_| Status::invalid_argument("invalid volume request"))?;
    timeout(IDLE, guest.write_all(&frame))
        .await
        .map_err(|_| timed_out())?
        .map_err(|_| malformed())
}

async fn next(
    input: &mut Streaming<VolumeReadRequest>,
    deadline: Duration,
) -> Result<VolumeReadRequest, Status> {
    timeout(deadline, input.message())
        .await
        .map_err(|_| timed_out())??
        .ok_or_else(|| Status::cancelled("volume transfer cancelled"))
}

async fn read_guest(guest: &mut DockerStream) -> Result<VolumeReadReply, Status> {
    timeout(IDLE, async {
        let mut header = [0; 4];
        guest
            .read_exact(&mut header)
            .await
            .map_err(|_| malformed())?;
        let size = volume_frame_length(header).map_err(|_| malformed())?;
        let mut payload = vec![0; size];
        guest
            .read_exact(&mut payload)
            .await
            .map_err(|_| malformed())?;
        decode_volume_reply(&payload).map_err(|_| malformed())
    })
    .await
    .map_err(|_| timed_out())?
}

async fn forward(out: &Output, reply: VolumeReadReply) -> Result<(), Status> {
    timeout(IDLE, out.send(Ok(reply)))
        .await
        .map_err(|_| timed_out())?
        .map_err(|_| Status::cancelled("volume transfer cancelled"))
}

fn malformed() -> Status {
    Status::data_loss("incomplete or invalid volume response")
}

fn timed_out() -> Status {
    Status::deadline_exceeded("volume transfer timed out")
}
