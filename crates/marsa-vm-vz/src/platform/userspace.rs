//! Run a user-space network stack behind an inherited, private datagram socket.

use std::{
    fs::{self, File},
    io,
    os::{
        fd::{AsRawFd as _, OwnedFd},
        unix::{fs::PermissionsExt as _, net::UnixDatagram},
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use marsa_vm::{VmError, VmState};
use tokio::{
    io::AsyncReadExt as _,
    process::Command,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use super::{native::Controller, state::backend};

#[cfg(test)]
mod tests;

const START: &str = "start user-space network helper";
const READY: &[u8; 6] = b"READY\n";
const START_LIMIT: Duration = Duration::from_secs(5);
pub(super) const GUEST_MAC: &str = "5a:94:ef:e4:0c:ee";

pub(super) struct Transport {
    helper: PathBuf,
    peer: File,
    session: Option<Session>,
}

struct Session {
    stop: oneshot::Sender<()>,
    worker: JoinHandle<()>,
}

impl Transport {
    pub(super) fn new(helper: &Path) -> Result<(Self, File), VmError> {
        let metadata = fs::symlink_metadata(helper).map_err(io_failure)?;
        let mode = metadata.permissions().mode();
        if !helper.is_absolute() || !metadata.is_file() || mode & 0o111 == 0 || mode & 0o6022 != 0 {
            return Err(backend(START, "expected a trusted regular executable"));
        }

        let (guest, peer) = UnixDatagram::pair().map_err(io_failure)?;
        configure_buffers(&guest).map_err(io_failure)?;
        configure_buffers(&peer).map_err(io_failure)?;
        let guest = File::from(OwnedFd::from(guest));
        let peer = File::from(OwnedFd::from(peer));
        Ok((
            Self {
                helper: helper.to_owned(),
                peer,
                session: None,
            },
            guest,
        ))
    }

    pub(super) async fn start(&mut self, controller: &Arc<Controller>) -> Result<(), VmError> {
        self.stop().await;
        self.clear_frames().map_err(io_failure)?;
        let input = self.peer.try_clone().map_err(io_failure)?;
        let mut child = Command::new(&self.helper)
            .env_clear()
            .stdin(Stdio::from(input))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(io_failure)?;

        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| backend(START, "missing readiness pipe"))?;
        let mut buffer = [0; READY.len()];
        timeout(START_LIMIT, output.read_exact(&mut buffer))
            .await
            .map_err(|_| VmError::Timeout {
                operation: START,
                limit: START_LIMIT,
            })?
            .map_err(|err| backend(START, format!("readiness pipe: {err}")))?;
        if &buffer != READY {
            return Err(backend(START, "invalid readiness signal"));
        }
        drop(output);
        if let Some(status) = child.try_wait().map_err(io_failure)? {
            return Err(backend(START, format!("helper exited: {status}")));
        }

        let (stop, shutdown) = oneshot::channel();
        let mut state = controller.lifecycle.watch();
        let controller = Arc::downgrade(controller);
        let worker = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = shutdown => { let _ = child.kill().await; }
                _ = terminal(&mut state) => { let _ = child.kill().await; }
                result = child.wait() => {
                    let detail = match result {
                        Ok(status) => format!("user-space network helper exited: {status}"),
                        Err(error) => format!("could not wait for the user-space network helper: {error}"),
                    };
                    if let Some(controller) = controller.upgrade() {
                        controller.submit(move |controller| controller.lifecycle.network_unavailable(detail));
                    }
                }
            }
        });
        self.session = Some(Session { stop, worker });
        Ok(())
    }

    pub(super) async fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.stop.send(());
            let _ = session.worker.await;
        }
    }

    fn clear_frames(&self) -> io::Result<()> {
        let mut buffer = [0; 2048];
        loop {
            // SAFETY: The descriptor is owned by self, and buffer is writable for its full length.
            let result = unsafe {
                libc::recv(
                    self.peer.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if result >= 0 {
                continue;
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(error);
        }
    }
}

async fn terminal(state: &mut watch::Receiver<VmState>) {
    // A stopped handle can start again. Observe a new transition before treating
    // Stopped as terminal for the newly spawned process.
    if state.changed().await.is_ok() {
        let _ = state
            .wait_for(|state| matches!(state, VmState::Stopped | VmState::Failed))
            .await;
    }
}

fn configure_buffers(socket: &UnixDatagram) -> io::Result<()> {
    for (option, size) in [
        (libc::SO_SNDBUF, 64_000_i32),
        (libc::SO_RCVBUF, 256_000_i32),
    ] {
        // SAFETY: The socket is borrowed and the integer matches the option's ABI.
        let status = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                option,
                std::ptr::from_ref(&size).cast(),
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn io_failure(source: io::Error) -> VmError {
    VmError::Io {
        operation: START,
        source,
    }
}
