//! Guest PID 1 and process supervision.

// Preserve the architecture's guest dependency edge for shared types.
use marsa_core as _;

#[cfg(target_os = "linux")]
mod platform;

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Err(error) = platform::run() {
        eprintln!("marsa-init: {error}");
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("marsa-init requires a Linux guest.");
    ExitCode::FAILURE
}
