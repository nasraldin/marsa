//! Guest process for the host control channel.

// Keep the architecture's dependency edges until this crate uses them directly.
use marsa_proto as _;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Report failure until this binary has a working entry point.
    ExitCode::FAILURE
}
