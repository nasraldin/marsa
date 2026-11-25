//! Native component gallery using the same controls as the Marsa desktop.

#[cfg(target_os = "macos")]
#[path = "gallery/native.rs"]
mod native;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    native::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {}
