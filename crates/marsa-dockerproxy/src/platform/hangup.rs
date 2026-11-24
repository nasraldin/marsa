//! Distinguish a full disconnect from write EOF reported by stream reads.
//! An upgraded client can finish sending stdin and still wait for stdout.

use std::{
    io,
    os::fd::{AsRawFd, RawFd},
};

use tokio::{
    io::{Interest, unix::AsyncFd},
    net::UnixStream,
};

pub(super) struct Hangup(imp::Queue);

impl AsRawFd for Hangup {
    fn as_raw_fd(&self) -> RawFd {
        imp::descriptor(&self.0)
    }
}

#[allow(unsafe_code)]
pub(super) fn watch(client: &UnixStream) -> io::Result<AsyncFd<Hangup>> {
    let queue = Hangup(imp::new(client)?);
    // SAFETY: The queue owns a stable descriptor until AsyncFd deregisters and drops it.
    unsafe { AsyncFd::register_with_interest(queue, Interest::READABLE) }
        .map_err(|error| error.into_parts().1)
}

pub(super) async fn closed(hangup: &AsyncFd<Hangup>) -> io::Result<()> {
    loop {
        let mut ready = hangup.readable().await?;
        match ready.try_io(|fd| imp::poll(&fd.get_ref().0)) {
            Ok(Ok(true)) => return Ok(()),
            Ok(Ok(false)) | Err(_) => continue,
            Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
            Ok(Err(error)) => return Err(error),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout};

    use super::*;

    pub(super) type Queue = Epoll;

    pub(super) fn descriptor(queue: &Queue) -> RawFd {
        queue.0.as_raw_fd()
    }

    pub(super) fn new(client: &UnixStream) -> io::Result<Queue> {
        let queue = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?;
        // EPOLLHUP and EPOLLERR are delivered even without a requested read/write event.
        queue.add(client, EpollEvent::new(EpollFlags::empty(), 0))?;
        Ok(queue)
    }

    pub(super) fn poll(queue: &Queue) -> io::Result<bool> {
        let mut events = [EpollEvent::empty()];
        match queue.wait(&mut events, EpollTimeout::ZERO)? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            _ => Ok(true),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod imp {
    use nix::{
        fcntl::{FcntlArg, FdFlag, fcntl},
        sys::{
            event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue},
            time::TimeSpec,
        },
    };

    use super::*;

    pub(super) type Queue = Kqueue;

    pub(super) fn descriptor(queue: &Queue) -> RawFd {
        std::os::fd::AsFd::as_fd(queue).as_raw_fd()
    }

    pub(super) fn new(client: &UnixStream) -> io::Result<Queue> {
        let queue = Kqueue::new()?;
        fcntl(&queue, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        let change = event(
            client.as_raw_fd() as usize,
            EvFlags::EV_ADD | EvFlags::EV_CLEAR,
        );
        queue.kevent(&[change], &mut [], Some(*TimeSpec::new(0, 0).as_ref()))?;
        Ok(queue)
    }

    pub(super) fn poll(queue: &Queue) -> io::Result<bool> {
        let mut events = [event(0, EvFlags::empty())];
        match queue.kevent(&[], &mut events, Some(*TimeSpec::new(0, 0).as_ref()))? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            // EVFILT_READ reports EOF even on shutdown(SHUT_WR). Watch the write filter instead.
            _ => Ok(events[0].flags().contains(EvFlags::EV_EOF)),
        }
    }

    fn event(fd: usize, flags: EvFlags) -> KEvent {
        KEvent::new(
            fd,
            EventFilter::EVFILT_WRITE,
            flags,
            FilterFlag::empty(),
            0,
            0,
        )
    }
}
