//! Host daemon for the virtual machine and its local clients.

mod diagnostics;
#[cfg(unix)]
mod platform;

// Keep the architecture's dependency edges until the daemon wires these services.
use marsa_net as _;
use marsa_vm_krun as _;
use marsa_vm_vz as _;

use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if !arguments.is_empty() {
        #[cfg(target_os = "macos")]
        if let [command, action] = arguments.as_slice()
            && command == "login"
        {
            return match platform::login(action) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("marsad: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        #[cfg(target_os = "macos")]
        eprintln!("{}", diagnostics::usage(diagnostics::LOGIN_USAGE));
        #[cfg(not(target_os = "macos"))]
        eprintln!("{}", diagnostics::usage("marsad"));
        return ExitCode::FAILURE;
    }
    #[cfg(unix)]
    match platform::run() {
        Ok(()) => return ExitCode::SUCCESS,
        Err(error) => eprintln!("marsad: {error}"),
    }
    #[cfg(not(unix))]
    eprintln!("marsad requires a Unix host");
    ExitCode::FAILURE
}
