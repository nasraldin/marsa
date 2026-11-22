//! Desktop application state and client orchestration.

#[cfg(unix)]
pub mod services;
#[cfg(unix)]
pub mod store;

#[cfg(target_os = "macos")]
mod desktop;
#[cfg(target_os = "macos")]
mod screens;

/// Open the native desktop workspace.
#[cfg(target_os = "macos")]
pub fn run() -> std::process::ExitCode {
    marsa_ui::run(|window, cx, locale| {
        use marsa_ui::gpui::AppContext as _;
        cx.new(|cx| desktop::Desktop::new(window, locale, cx))
    })
}
