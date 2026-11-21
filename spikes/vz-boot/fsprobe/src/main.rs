//! Measure the same filesystem operations on macOS and inside the Linux guest.

mod bench;
#[cfg(target_os = "linux")]
mod platform;

use std::env;
use std::io;
use std::path::PathBuf;

fn main() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let command = args.next().and_then(|value| value.into_string().ok());
    let root = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(usage());
    }
    match (command.as_deref(), root) {
        (Some("bench"), Some(root)) => bench::run(&root),
        #[cfg(target_os = "linux")]
        (Some("watch"), Some(root)) => platform::watch(&root),
        #[cfg(target_os = "linux")]
        (Some("create-unprivileged"), Some(root)) => platform::create_unprivileged(&root),
        #[cfg(target_os = "linux")]
        (Some("xattrs"), Some(root)) => platform::xattrs(&root),
        _ => Err(usage()),
    }
}

fn usage() -> io::Error {
    io::Error::other(
        "usage: marsa-virtiofs-spike <bench|watch|create-unprivileged|xattrs> <fixture>",
    )
}
