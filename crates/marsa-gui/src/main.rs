//! Marsa desktop client.

use marsa_client as _;
use std::process::ExitCode;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    marsa_ui::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    ExitCode::FAILURE
}
