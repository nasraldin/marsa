//! Marsa desktop client.

use std::process::ExitCode;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    marsa_gui::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    ExitCode::FAILURE
}
