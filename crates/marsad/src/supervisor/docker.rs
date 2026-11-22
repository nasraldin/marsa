//! Keep a Docker connection accounted for from boot request through relay shutdown.

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use marsa_vm::VsockStream;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::watch,
};

pub(super) struct Activity(watch::Sender<usize>);

impl Activity {
    pub(super) fn acquire(clients: &watch::Sender<usize>) -> Self {
        clients.send_modify(|count| *count += 1);
        Self(clients.clone())
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}

/// A Docker API stream that keeps the current engine active until the stream closes.
pub struct DockerStream {
    pub(super) stream: VsockStream,
    pub(super) _activity: Activity,
}

impl AsyncRead for DockerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}

impl AsyncWrite for DockerStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
