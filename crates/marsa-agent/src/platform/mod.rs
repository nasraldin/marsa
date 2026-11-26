//! Linux-only AF_VSOCK and guest clock operations.

mod authorization;
mod bridge;
mod network;
mod network_facts;
mod operations;
mod volume_files;
mod vsock;

use std::io;

use marsa_proto::guest::{
    BIND_AUTHORIZATION_PORT, CONTROL_PORT, DOCKER_API_PORT, VOLUME_FILES_PORT,
    v1::{
        ConfigureNetwork, EgressResult, InstallCertificates, PortEvent, ProbeEgress, SetTime,
        VmStats, guest_envelope, host_envelope,
    },
};
use nix::{
    sys::time::TimeSpec,
    time::{ClockId, clock_settime},
};
use tokio::{fs::OpenOptions, sync::mpsc, task::JoinSet};

use crate::{
    authorization::socket::GUEST_ENGINE_SOCKET,
    control::{self, GuestState},
    metadata,
};

const READY_FILE: &str = "/run/marsa/agent.ready";

struct Linux {
    network: network::Config,
}

impl GuestState for Linux {
    async fn configure_network(&self, request: ConfigureNetwork) -> io::Result<bool> {
        self.network.configure(request).await
    }

    async fn install_certificates(&self, request: InstallCertificates) -> io::Result<bool> {
        self.network.certificates(request).await
    }

    fn set_time(&self, time: SetTime) -> io::Result<()> {
        if time.unix_seconds < 0 || time.nanoseconds >= 1_000_000_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid guest wall clock",
            ));
        }
        clock_settime(
            ClockId::CLOCK_REALTIME,
            TimeSpec::new(time.unix_seconds, time.nanoseconds.into()),
        )?;
        Ok(())
    }

    async fn mark_ready(&self) -> io::Result<()> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(READY_FILE)
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn engine_released(&self) -> bool {
        // An unreadable marker is not proof that the engine has never started.
        !matches!(tokio::fs::symlink_metadata(READY_FILE).await, Err(error) if error.kind() == io::ErrorKind::NotFound)
    }

    async fn probe_egress(&self, request: ProbeEgress) -> EgressResult {
        crate::egress::probe(request).await
    }

    async fn command(&self, body: host_envelope::Body) -> io::Result<Option<guest_envelope::Body>> {
        operations::command(body).await
    }

    async fn stats(&self) -> io::Result<Option<VmStats>> {
        operations::stats().await.map(Some)
    }

    fn start_port_events(
        &self,
        path: &str,
        output: mpsc::Sender<PortEvent>,
        tasks: &mut JoinSet<()>,
    ) {
        let path = path.to_owned();
        tasks.spawn(async move { operations::ports::watch(&path, output).await });
    }

    fn shutdown(&self) -> io::Result<()> {
        operations::shutdown()
    }
}

pub(crate) fn run() -> io::Result<()> {
    let hello = metadata::hello()?;
    network::native::configure()?;
    let state = Linux {
        network: network::Config::load()?,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let listener = vsock::Listener::bind(CONTROL_PORT)?;
            let bridge = vsock::Listener::bind(DOCKER_API_PORT)?;
            let facts = vsock::Listener::bind(marsa_proto::guest::NETWORK_FACTS_PORT)?;
            let bind_listener = vsock::Listener::bind(BIND_AUTHORIZATION_PORT)?;
            // The optional provider must not take the Docker engine or control channel down.
            if let Ok(volumes) = vsock::Listener::bind(VOLUME_FILES_PORT) {
                tokio::spawn(volume_files::serve(volumes));
            }
            let plugin = authorization::bind_plugin()?;
            let policy = crate::authorization::Policy::default();
            tokio::spawn(authorization::serve_host(bind_listener, policy.clone()));
            tokio::spawn(authorization::serve_plugin(plugin, policy));
            let interface = network_facts::interface()?;
            tokio::spawn(network_facts::serve(facts, interface));
            tokio::spawn(async move { bridge::serve(&bridge, GUEST_ENGINE_SOCKET).await });
            loop {
                let stream = listener.accept_host().await?;
                if let Err(error) =
                    control::session(stream, hello.clone(), &state, GUEST_ENGINE_SOCKET).await
                    && error.kind() != io::ErrorKind::UnexpectedEof
                {
                    eprintln!("marsa-agent: control connection: {error}");
                }
            }
        })
}
