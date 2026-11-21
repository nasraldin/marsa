//! Serve the guest control socket and report Docker engine state to the host.

#[cfg(any(target_os = "linux", test))]
mod control;
#[cfg(target_os = "linux")]
mod egress;
#[cfg(any(target_os = "linux", test))]
mod health;
#[cfg(target_os = "linux")]
mod metadata;
#[cfg(target_os = "linux")]
mod platform;

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Err(error) = platform::run() {
        eprintln!("marsa-agent: {error}");
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("marsa-agent requires a Linux guest.");
    ExitCode::FAILURE
}
