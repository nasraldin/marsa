//! Marsa desktop client.

use std::process::ExitCode;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    let _services = marsa_gui::services::Services::start();
    marsa_ui::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    ExitCode::FAILURE
}
