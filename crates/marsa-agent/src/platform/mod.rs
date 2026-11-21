//! Linux-only AF_VSOCK and guest clock operations.

mod network;
mod vsock;

use std::io;

use marsa_proto::guest::{
    CONTROL_PORT,
    v1::{ConfigureNetwork, InstallCertificates, SetTime},
};
use nix::{
    sys::time::TimeSpec,
    time::{ClockId, clock_settime},
};
use tokio::fs::OpenOptions;

use crate::{
    control::{self, GuestState},
    metadata,
};

const ENGINE_SOCKET: &str = "/var/run/docker.sock";
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
}

pub(crate) fn run() -> io::Result<()> {
    let hello = metadata::hello()?;
    let state = Linux {
        network: network::Config::load()?,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let listener = vsock::Listener::bind(CONTROL_PORT)?;
            loop {
                let stream = listener.accept_host().await?;
                if let Err(error) =
                    control::session(stream, hello.clone(), &state, ENGINE_SOCKET).await
                    && error.kind() != io::ErrorKind::UnexpectedEof
                {
                    eprintln!("marsa-agent: control connection: {error}");
                }
            }
        })
}
