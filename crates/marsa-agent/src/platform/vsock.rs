//! Register a nonblocking Linux vsock listener and streams with Tokio.

#![allow(unsafe_code)]

use std::{
    io,
    os::fd::{AsRawFd, OwnedFd},
    pin::Pin,
    task::{Context, Poll, ready},
};

use nix::{
    errno::Errno,
    libc::{VMADDR_CID_ANY, VMADDR_CID_HOST},
    sys::socket::{
        AddressFamily, Backlog, MsgFlags, Shutdown, SockFlag, SockType, VsockAddr, accept4, bind,
        getpeername, listen, send, shutdown, socket,
    },
    unistd::read,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd};

const FLAGS: SockFlag = SockFlag::SOCK_NONBLOCK.union(SockFlag::SOCK_CLOEXEC);

pub(super) struct Listener(AsyncFd<OwnedFd>);

impl Listener {
    pub(super) fn bind(port: u32) -> io::Result<Self> {
        let socket = socket(AddressFamily::Vsock, SockType::Stream, FLAGS, None)?;
        bind(socket.as_raw_fd(), &VsockAddr::new(VMADDR_CID_ANY, port))?;
        listen(&socket, Backlog::new(1)?)?;
        Ok(Self(register(socket)?))
    }

    pub(super) async fn accept_host(&self) -> io::Result<Stream> {
        loop {
            let mut ready = self.0.readable().await?;
            let Ok(socket) = ready
                .try_io(|listener| accept4(listener.get_ref(), FLAGS).map_err(io::Error::from))
            else {
                continue;
            };
            let socket = socket?;
            let peer: VsockAddr = getpeername(socket.as_raw_fd())?;
            if peer.cid() == VMADDR_CID_HOST {
                return Ok(Stream(register(socket)?));
            }
        }
    }
}

fn register(socket: OwnedFd) -> io::Result<AsyncFd<OwnedFd>> {
    // SAFETY: The OwnedFd was created by socket or accept4 with nonblocking
    // mode. AsyncFd takes exclusive ownership and keeps its descriptor open
    // and unchanged until deregistration. The listener runs inside Tokio.
    Ok(unsafe { AsyncFd::register(socket)? })
}

pub(super) struct Stream(AsyncFd<OwnedFd>);

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut ready = ready!(self.0.poll_read_ready(cx))?;
            if let Ok(result) = ready.try_io(|socket| {
                read(socket.get_ref(), buffer.initialize_unfilled()).map_err(io::Error::from)
            }) {
                buffer.advance(result?);
                return Poll::Ready(Ok(()));
            }
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            let mut ready = ready!(self.0.poll_write_ready(cx))?;
            if let Ok(result) = ready.try_io(|socket| {
                send(socket.get_ref().as_raw_fd(), bytes, MsgFlags::MSG_NOSIGNAL)
                    .map_err(io::Error::from)
            }) {
                return Poll::Ready(result);
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match shutdown(self.0.get_ref().as_raw_fd(), Shutdown::Write) {
            Ok(()) | Err(Errno::ENOTCONN) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(error.into())),
        }
    }
}
