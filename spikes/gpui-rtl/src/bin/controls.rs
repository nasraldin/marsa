//! Drive native Kit controls using the same Metal-backed window as the RTL spike.

#[cfg(target_os = "macos")]
extern crate gpui_kit as gpui;

#[cfg(target_os = "macos")]
#[path = "controls/mod.rs"]
mod controls;

#[cfg(target_os = "macos")]
fn main() -> gpui_kit::Result<()> {
    controls::measure()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("M1-06 requires macOS. يتطلب هذا الاختبار نظام macOS.");
    std::process::ExitCode::FAILURE
}
