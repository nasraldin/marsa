//! Adapt a duplicated VZ descriptor to Tokio while its native owner stays on the VM queue.

use std::{
    io,
    os::{fd::OwnedFd, unix::net::UnixStream as StdUnixStream},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use marsa_vm::{VmError, VsockStream};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::UnixStream,
    sync::oneshot,
};

use super::{native::Controller, state::backend};

pub(super) const CONNECT: &str = "connect vsock";
pub(super) type Reply = oneshot::Sender<Result<Connected, VmError>>;

pub(super) struct Connected {
    pub(super) fd: OwnedFd,
    pub(super) lease: Lease,
}

pub(super) struct Lease {
    pub(super) controller: Arc<Controller>,
    pub(super) id: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let id = self.id;
        self.controller
            .submit(move |controller| controller.release_socket(id));
    }
}

struct Transport {
    stream: UnixStream,
    _lease: Lease,
}

pub(super) async fn connect(
    controller: &Arc<Controller>,
    port: u32,
) -> Result<VsockStream, VmError> {
    let (reply, result) = oneshot::channel();
    controller.submit(move |controller| controller.connect_socket(port, reply));
    let Connected { fd, lease } = result
        .await
        .map_err(|_| backend(CONNECT, "the VZ queue lost the socket result"))??;
    let stream = StdUnixStream::from(fd);
    let stream = stream
        .set_nonblocking(true)
        .and_then(|()| UnixStream::from_std(stream))
        .map_err(io_error)?;
    Ok(Box::new(Transport {
        stream,
        _lease: lease,
    }))
}

pub(super) fn io_error(source: io::Error) -> VmError {
    VmError::Io {
        operation: CONNECT,
        source,
    }
}

impl AsyncRead for Transport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}

impl AsyncWrite for Transport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, buffers)
    }
}
