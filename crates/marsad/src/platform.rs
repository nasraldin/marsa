//! Run the Unix daemon and keep its transport and files scoped to the current user.

mod api;
mod files;
mod logging;
#[cfg(target_os = "macos")]
mod login;

use std::{error::Error, io, time::Duration};

use marsa_core::paths::MarsaPaths;
use nix::{
    sys::stat::{Mode, umask},
    unistd::{Uid, geteuid},
};
use tokio::{
    signal::unix::{Signal, SignalKind, signal},
    sync::oneshot,
    time::timeout,
};

type DaemonError = Box<dyn Error + Send + Sync>;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn run() -> Result<(), DaemonError> {
    let uid = require_unprivileged()?;
    // Set the daemon's private file-creation policy before starting worker threads.
    umask(Mode::S_IRWXG | Mode::S_IRWXO);
    let home =
        std::env::home_dir().ok_or_else(|| io::Error::other("could not find the user home"))?;
    let paths = MarsaPaths::from_home(home)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let signals = {
        let _entered = runtime.enter();
        Signals::register()?
    };

    let uid = uid.as_raw();
    let mut instance = files::Instance::acquire(&paths, uid)?;
    let _settings = files::read_settings(&paths, uid)?;
    let _logging = logging::initialize(&paths, uid)?;
    let socket = instance.publish(&paths, uid)?;
    let serving = runtime.block_on(async {
        let socket = tokio::net::UnixListener::from_std(socket)?;
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "daemon started");
        serve(socket, uid, signals).await
    });
    let cleanup = instance.cleanup();
    serving?;
    cleanup?;
    tracing::info!("daemon stopped");
    Ok(())
}

fn require_unprivileged() -> io::Result<Uid> {
    let uid = geteuid();
    if uid.is_root() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            crate::diagnostics::localized(
                "marsad must run as an unprivileged user",
                "يجب تشغيل marsad بحساب مستخدم عادي",
            ),
        ));
    }
    Ok(uid)
}

#[cfg(target_os = "macos")]
pub(super) fn login(action: &std::ffi::OsStr) -> io::Result<()> {
    require_unprivileged()?;
    login::run(action)
}

async fn serve(
    listener: tokio::net::UnixListener,
    uid: u32,
    signals: Signals,
) -> Result<(), DaemonError> {
    let (shutdown, stop) = oneshot::channel();
    let server = api::serve(listener, uid, async {
        let _ = stop.await;
    });
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result?,
        signal = signals.wait() => {
            tracing::info!(signal, "daemon shutdown requested");
            let _ = shutdown.send(());
            timeout(SHUTDOWN_TIMEOUT, &mut server).await??;
        }
    }
    Ok(())
}

struct Signals {
    terminate: Signal,
    interrupt: Signal,
}

impl Signals {
    fn register() -> io::Result<Self> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    async fn wait(mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
        }
    }
}
