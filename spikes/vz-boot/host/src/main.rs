//! Drive both Virtualization.framework bridges through the same bounded boot test.

mod native;
mod runner;
#[cfg(feature = "swift")]
mod swift;

use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use runner::Backend;

fn main() -> ExitCode {
    match objc2::rc::autoreleasepool(|_| run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("marsa-vz-spike: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1).peekable();
    let writable_share = args.next_if(|arg| arg == "--writable-share").is_some();
    let via_init = args.next_if(|arg| arg == "--via-init").is_some();
    let usage = || {
        io::Error::other(
            "usage: marsa-vz-spike [--writable-share] [--via-init] <rust|swift> <fixture-directory> <console-log> [guest-script]",
        )
    };
    let backend = match args
        .next()
        .and_then(|name| name.into_string().ok())
        .as_deref()
    {
        Some("rust") => Backend::Rust,
        #[cfg(feature = "swift")]
        Some("swift") => Backend::Swift,
        #[cfg(not(feature = "swift"))]
        Some("swift") => return Err(io::Error::other("build with --features swift")),
        _ => return Err(usage()),
    };
    #[cfg(feature = "swift")]
    if via_init && matches!(backend, Backend::Swift) {
        return Err(io::Error::other("--via-init requires the Rust bridge"));
    }
    let fixtures = PathBuf::from(args.next().ok_or_else(usage)?).canonicalize()?;
    if fixtures.to_str().is_none() {
        return Err(io::Error::other("fixture paths must be UTF-8"));
    }
    let console = PathBuf::from(args.next().ok_or_else(usage)?);
    let script = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(usage());
    }
    runner::run(
        backend,
        &fixtures,
        &console,
        script.as_deref(),
        writable_share,
        via_init,
    )
}
