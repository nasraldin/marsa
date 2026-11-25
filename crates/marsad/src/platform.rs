//! Run the Unix daemon and keep its transport and files scoped to the current user.

#[cfg(test)]
mod crash_tests;
mod files;
mod logging;
#[cfg(target_os = "macos")]
mod login;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod menu_bar;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod runtime;

use std::{error::Error, io, time::Duration};

use marsa_core::paths::MarsaPaths;
use marsad::typed_api::TypedApi;
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
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
// Cover the packaged runtime's original attempt, cleanup and bounded retry.
const DOCKER_START_TIMEOUT: Duration = Duration::from_secs(180);

pub(super) fn run() -> Result<(), DaemonError> {
    let uid = require_unprivileged()?;
    // Set the daemon's private file-creation policy before starting worker threads.
    umask(Mode::S_IRWXG | Mode::S_IRWXO);
    let home =
        std::env::home_dir().ok_or_else(|| io::Error::other("could not find the user home"))?;
    let paths = MarsaPaths::from_home(&home)?;
    let uid = uid.as_raw();
    // Acquire the daemon lock before creating a status item. A losing
    // launcher has no AppKit work and never briefly shows a second item.
    let instance = files::Instance::acquire(&paths, uid)?;
    let prepared = Prepared {
        uid,
        home,
        paths,
        instance,
    };
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        if let Some(bundle) =
            runtime::Packaged::discover()?.and_then(|package| package.menu_bar_bundle())
        {
            return menu_bar::run(bundle, move |ui| run_daemon(prepared, ui));
        }
        run_daemon(prepared, None)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    run_daemon(prepared)
}

struct Prepared {
    uid: u32,
    home: std::path::PathBuf,
    paths: MarsaPaths,
    instance: files::Instance,
}

fn run_daemon(
    prepared: Prepared,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))] mut ui: Option<menu_bar::DaemonUi>,
) -> Result<(), DaemonError> {
    let Prepared {
        uid,
        home,
        paths,
        mut instance,
    } = prepared;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    drop(home);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let signals = {
        let _entered = runtime.enter();
        Signals::register()?
    };

    marsad::factory::recover(&paths, uid)?;
    let settings = files::read_settings(&paths, uid)?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let vm_settings = settings.vm.clone();
    let api = TypedApi::new(paths.clone(), uid, settings)?;
    let _logging = logging::initialize(&paths, uid)?;
    let (socket, docker, agent) = instance.publish(&paths, uid)?;
    #[cfg(target_os = "macos")]
    let (_watcher, host_events) = match marsad::host_events::platform::Watcher::start() {
        Ok((watcher, events)) => (Some(watcher), Some(events)),
        Err(error) => {
            tracing::warn!(%error, "host notifications unavailable");
            (None, None)
        }
    };
    let serving = runtime.block_on(async {
        let socket = tokio::net::UnixListener::from_std(socket)?;
        let docker = tokio::net::UnixListener::from_std(docker)?;
        let agent = tokio::net::UnixListener::from_std(agent)?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let api = configure_packaged_runtime(api, paths, uid, vm_settings, &home).await?;
        #[cfg(target_os = "macos")]
        if let Some(events) = host_events {
            let api = api.clone();
            tokio::spawn(async move { api.watch_host_events(events).await });
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let quit = ui.take().map(|mut ui| {
            ui.attach(&api);
            ui.quit
        });
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let quit = None;
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "daemon started");
        serve(socket, docker, agent, api, uid, signals, quit).await
    });
    // A canceled RPC may leave a durability worker in spawn_blocking. Keep
    // the instance lock until the runtime has joined any such worker.
    drop(runtime);
    let cleanup = instance.cleanup();
    serving?;
    cleanup?;
    tracing::info!("daemon stopped");
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
async fn configure_packaged_runtime(
    api: TypedApi,
    paths: MarsaPaths,
    uid: u32,
    settings: marsa_core::settings::VmSettings,
    home: &std::path::Path,
) -> io::Result<TypedApi> {
    use marsad::{guest_control::GuestControl, supervisor::SupervisorConfig};

    let Some(package) = runtime::Packaged::discover()? else {
        return Ok(api);
    };
    let package = std::sync::Arc::new(package);
    let docker_bundle = package.clone();
    let keychain_bundle = package.clone();
    let docker_config = std::env::var_os("DOCKER_CONFIG")
        .filter(|override_path| !override_path.is_empty())
        .map_or_else(|| home.join(".docker"), std::path::PathBuf::from);
    let api = api
        .with_docker_contexts(docker_config, move || docker_bundle.docker_cli())
        .with_registry_keychain(move || keychain_bundle.verify_registry_keychain());
    let config = SupervisorConfig {
        // A fresh default disk can spend about 26 seconds in the guest blank
        // check and initialization before DHCP and the control agent start.
        boot_timeout: Duration::from_secs(75),
        stop_timeout: Duration::from_secs(8),
        idle_timeout: Duration::from_secs(u64::from(settings.idle_timeout)),
        idle_stop: settings.idle_stop,
        max_retries: 1,
        initial_backoff: Duration::from_secs(1),
        maximum_backoff: Duration::from_secs(4),
    };
    let versions = package.component_versions().unwrap_or_else(|error| {
        tracing::warn!(kind = ?error.kind(), "could not read bundled guest versions");
        Vec::new()
    });
    api.with_components(versions)
        .with_packaged_runtime(
            package.capabilities(),
            GuestControl::default().with_agent_connect_timeout(Duration::from_secs(50)),
            config,
            move |desired, policy| package.prepare(&paths, uid, desired, policy),
        )
        .await
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
    docker: tokio::net::UnixListener,
    agent: tokio::net::UnixListener,
    api: TypedApi,
    uid: u32,
    signals: Signals,
    quit: Option<oneshot::Receiver<()>>,
) -> Result<(), DaemonError> {
    let (shutdown, stop) = oneshot::channel();
    let server = api.serve(listener, async {
        let _ = stop.await;
    });
    let agents = api.serve_agents(agent);
    tokio::pin!(server, agents);
    let relay = {
        let docker_api = api.clone();
        let startup = marsa_dockerproxy::Startup {
            timeout: DOCKER_START_TIMEOUT,
            timed_out: crate::diagnostics::localized(
                "Marsa timed out waiting for the Docker engine to start. Check Marsa's daemon logs and retry.",
                "انتهت مهلة انتظار مارسا لبدء محرك دوكر. راجع سجلات خدمة مارسا وأعد المحاولة.",
            ),
            unavailable: crate::diagnostics::localized(
                "Marsa could not connect to the Docker engine. Check Marsa's daemon logs and retry.",
                "تعذر على مارسا الاتصال بمحرك دوكر. راجع سجلات خدمة مارسا وأعد المحاولة.",
            ),
        };
        let proxy = marsa_dockerproxy::serve(docker, uid, startup, move || {
            let api = docker_api.clone();
            async move { api.connect_docker().await }
        });
        tokio::pin!(proxy);
        tokio::select! {
            result = &mut server => return result.map_err(Into::into),
            result = &mut agents => result.map_err(io::Error::other),
            result = &mut proxy => result,
            signal = signals.wait(quit) => {
                tracing::info!(signal, "daemon shutdown requested");
                Ok(())
            }
        }
    };
    let _ = shutdown.send(());
    timeout(SHUTDOWN_TIMEOUT, &mut server).await??;
    relay?;
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

    async fn wait(mut self, quit: Option<oneshot::Receiver<()>>) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
            _ = async {
                if let Some(quit) = quit && quit.await.is_ok() {
                    return;
                }
                std::future::pending::<()>().await;
            } => "menu bar",
        }
    }
}
