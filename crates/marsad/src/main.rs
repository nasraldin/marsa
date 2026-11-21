//! Host daemon for the virtual machine and its local clients.

#[cfg(unix)]
mod platform;

// Keep the architecture's dependency edges until the daemon wires these services.
use marsa_dockerproxy as _;
use marsa_net as _;
use marsa_share as _;
use marsa_vm_krun as _;
use marsa_vm_vz as _;

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(unix)]
    match platform::run() {
        Ok(()) => return ExitCode::SUCCESS,
        Err(error) => eprintln!("marsad: {error}"),
    }
    #[cfg(not(unix))]
    eprintln!("marsad requires a Unix host");
    ExitCode::FAILURE
}
