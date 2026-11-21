//! Exercises GPUI's macOS renderer and input path for the Marsa RTL gate.

#[cfg(target_os = "macos")]
mod evidence;
#[cfg(target_os = "macos")]
mod view;

#[cfg(target_os = "macos")]
fn main() -> gpui_kit::Result<()> {
    use std::ffi::OsStr;
    use std::io;
    use std::path::PathBuf;

    let mut args = std::env::args_os().skip(1);
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some(flag), None, None) if flag == OsStr::new("--interactive") => {
            evidence::interactive();
            Ok(())
        }
        (Some(flag), Some(destination), None) if flag == OsStr::new("--capture") => {
            evidence::capture(&PathBuf::from(destination))
        }
        _ => Err(io::Error::other(
            "usage: marsa-gpui-rtl-spike --capture DIRECTORY | --interactive",
        )
        .into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("The M1-01 spike requires a macOS Metal renderer.");
    std::process::ExitCode::FAILURE
}
