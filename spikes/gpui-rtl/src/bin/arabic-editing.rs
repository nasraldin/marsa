//! Compares CoreText caret geometry with the GPUI Kit input integration.

#[cfg(target_os = "macos")]
#[path = "arabic_editing/core_text.rs"]
mod core_text;
#[cfg(target_os = "macos")]
#[path = "arabic_editing/editor.rs"]
mod editor;

#[cfg(target_os = "macos")]
#[path = "arabic_editing/platform.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "arabic_editing/grapheme_view.rs"]
mod grapheme_view;

#[cfg(target_os = "macos")]
fn main() -> gpui_kit::Result<()> {
    core_text::probe()?;
    editor::probe()?;
    platform::probe()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("Arabic editing qualification requires macOS.");
    std::process::ExitCode::FAILURE
}
