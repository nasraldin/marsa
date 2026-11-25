//! Serve a confined read, on a host-only connection, after the owner chooses a destination.

mod confinement;

use std::{io, time::Duration};

use http_body_util::{BodyExt as _, Empty, Limited};
use hyper::{Request, StatusCode, body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use marsa_proto::guest::{
    VOLUME_READ_CHUNK, VOLUME_READ_LIMIT, decode_volume_request, encode_volume_reply,
    v1::{
        SelectVolumeRead, VolumeReadFailure as Failure, VolumeReadReply,
        volume_read_reply::Body as Reply, volume_read_request::Body as RequestBody,
    },
    volume_frame_length,
};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::mpsc,
    task::spawn_blocking,
    time::timeout,
};

use crate::authorization::socket::GUEST_ENGINE_SOCKET;

use super::vsock::{Listener, Stream};
use confinement::SelectedVolume;

const FRAME_TIMEOUT: Duration = Duration::from_secs(60);
const OWNER_INPUT_TIMEOUT: Duration = Duration::from_secs(180);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);
const INSPECT_LIMIT: usize = 64 * 1024;

pub(super) async fn serve(listener: Listener) {
    while let Ok(mut stream) = listener.accept_host().await {
        let result = timeout(TRANSFER_TIMEOUT, session(&mut stream)).await;
        if let Ok(Err(failure)) = result {
            let _ = send(&mut stream, Reply::Failure(failure as i32)).await;
        }
    }
}

async fn session(stream: &mut Stream) -> Result<(), Failure> {
    let request = match receive(stream, FRAME_TIMEOUT).await? {
        RequestBody::Probe(true) => return send(stream, Reply::Capable(true)).await,
        RequestBody::Select(request) => request,
        _ => return Err(Failure::InvalidPath),
    };
    let info = inspect(&request).await?;
    let volume = spawn_blocking({
        let request = request.clone();
        move || SelectedVolume::open(&request)
    })
    .await
    .map_err(|_| Failure::Io)?
    .map_err(failure)?;
    if inspect(&request).await? != info {
        return Err(Failure::Changed);
    }
    send(stream, Reply::Selected(true)).await?;
    let RequestBody::Prepare(path) = receive(stream, OWNER_INPUT_TIMEOUT).await? else {
        return Err(Failure::InvalidPath);
    };
    if inspect(&request).await? != info {
        return Err(Failure::Changed);
    }
    let selection = spawn_blocking(move || volume.prepare(&path))
        .await
        .map_err(|_| Failure::Io)?
        .map_err(failure)?;
    send(stream, Reply::Format(selection.format() as i32)).await?;

    if !matches!(
        receive(stream, OWNER_INPUT_TIMEOUT).await?,
        RequestBody::Start(true)
    ) {
        return Err(Failure::InvalidPath);
    }
    if inspect(&request).await? != info {
        return Err(Failure::Changed);
    }

    let (chunks, mut data) = mpsc::channel(2);
    let producer = spawn_blocking(move || {
        let mut output = ChunkWriter { chunks, bytes: 0 };
        selection.write(&mut output)?;
        Ok::<_, io::Error>(selection)
    });
    while let Some(bytes) = data.recv().await {
        send(stream, Reply::Data(bytes)).await?;
    }
    let selection = producer.await.map_err(|_| Failure::Io)?.map_err(failure)?;
    if inspect(&request).await? != info {
        return Err(Failure::Changed);
    }
    spawn_blocking(move || selection.revalidate())
        .await
        .map_err(|_| Failure::Io)?
        .map_err(failure)?;
    send(stream, Reply::Complete(true)).await
}

async fn receive(stream: &mut Stream, deadline: Duration) -> Result<RequestBody, Failure> {
    timeout(deadline, async {
        let mut header = [0; 4];
        stream
            .read_exact(&mut header)
            .await
            .map_err(|_| Failure::Io)?;
        let length = volume_frame_length(header).map_err(|_| Failure::InvalidPath)?;
        let mut payload = vec![0; length];
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|_| Failure::Io)?;
        decode_volume_request(&payload)
            .map_err(|_| Failure::InvalidPath)?
            .body
            .ok_or(Failure::InvalidPath)
    })
    .await
    .map_err(|_| Failure::Io)?
}

async fn send(stream: &mut Stream, reply: Reply) -> Result<(), Failure> {
    let bytes =
        encode_volume_reply(&VolumeReadReply { body: Some(reply) }).map_err(|_| Failure::Io)?;
    timeout(FRAME_TIMEOUT, stream.write_all(&bytes))
        .await
        .map_err(|_| Failure::Io)?
        .map_err(|_| Failure::Io)
}

#[derive(Deserialize, Eq, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct VolumeInfo {
    name: String,
    driver: String,
    mountpoint: String,
    created_at: Option<String>,
    scope: String,
    options: Option<std::collections::BTreeMap<String, String>>,
}

async fn inspect(request: &SelectVolumeRead) -> Result<VolumeInfo, Failure> {
    timeout(INSPECT_TIMEOUT, async {
        let socket = UnixStream::connect(GUEST_ENGINE_SOCKET)
            .await
            .map_err(|_| Failure::Io)?;
        let (mut client, connection) = http1::handshake(TokioIo::new(socket))
            .await
            .map_err(|_| Failure::Io)?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let input = Request::builder()
            .uri(format!("/v1.57/volumes/{}", request.name))
            .header("host", "docker")
            .header("connection", "close")
            .body(Empty::<Bytes>::new())
            .map_err(|_| Failure::Io)?;
        let response = client.send_request(input).await.map_err(|_| Failure::Io)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::NOT_FOUND => return Err(Failure::Changed),
            _ => return Err(Failure::Io),
        }
        let bytes = Limited::new(response.into_body(), INSPECT_LIMIT)
            .collect()
            .await
            .map_err(|_| Failure::Io)?
            .to_bytes();
        let info: VolumeInfo = serde_json::from_slice(&bytes).map_err(|_| Failure::Io)?;
        info.check(request)?;
        Ok(info)
    })
    .await
    .map_err(|_| Failure::Io)?
}

impl VolumeInfo {
    fn check(&self, request: &SelectVolumeRead) -> Result<(), Failure> {
        if self.name != request.name || self.created_at.as_ref() != Some(&request.created_at) {
            return Err(Failure::Changed);
        }
        if self.driver != "local"
            || self.scope != "local"
            || self
                .options
                .as_ref()
                .is_some_and(|options| !options.is_empty())
            || self.mountpoint != format!("/var/lib/docker/volumes/{}/_data", request.name)
        {
            return Err(Failure::Unsupported);
        }
        Ok(())
    }
}

struct ChunkWriter {
    chunks: mpsc::Sender<Vec<u8>>,
    bytes: u64,
}

impl io::Write for ChunkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len().min(VOLUME_READ_CHUNK);
        if self.bytes + length as u64 > VOLUME_READ_LIMIT {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        if length != 0 {
            self.chunks
                .blocking_send(bytes[..length].to_vec())
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            self.bytes += length as u64;
        }
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn failure(error: io::Error) -> Failure {
    match error.kind() {
        io::ErrorKind::Unsupported => Failure::Unsupported,
        io::ErrorKind::NotFound | io::ErrorKind::StaleNetworkFileHandle => Failure::Changed,
        io::ErrorKind::FileTooLarge => Failure::Limit,
        io::ErrorKind::InvalidInput => Failure::InvalidPath,
        _ if error.raw_os_error() == Some(nix::libc::ENOSYS) => Failure::Unsupported,
        _ if matches!(
            error.raw_os_error(),
            Some(nix::libc::ELOOP | nix::libc::EXDEV)
        ) =>
        {
            Failure::InvalidPath
        }
        _ => Failure::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_the_exact_option_free_local_volume() {
        let request = SelectVolumeRead {
            name: "data".into(),
            created_at: "2026-11-26T00:00:00Z".into(),
        };
        let mut info = VolumeInfo {
            name: request.name.clone(),
            driver: "local".into(),
            scope: "local".into(),
            mountpoint: "/var/lib/docker/volumes/data/_data".into(),
            created_at: Some(request.created_at.clone()),
            options: None,
        };
        assert_eq!(info.check(&request), Ok(()));
        info.options = Some([("device".into(), "/etc".into())].into());
        assert_eq!(info.check(&request), Err(Failure::Unsupported));
        info.options = None;
        info.mountpoint = "/var/lib/docker/volumes/other/_data".into();
        assert_eq!(info.check(&request), Err(Failure::Unsupported));
        info.created_at = None;
        assert_eq!(info.check(&request), Err(Failure::Changed));
    }
}
