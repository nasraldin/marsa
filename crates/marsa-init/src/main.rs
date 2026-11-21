//! Guest PID 1 and process supervision.

// Keep the architecture's dependency edges until this crate uses them directly.
use marsa_core as _;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Report failure until this binary has a working entry point.
    ExitCode::FAILURE
}
