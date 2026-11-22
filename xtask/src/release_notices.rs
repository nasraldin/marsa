//! Stage releases' notice files atomically after generating them with Python.

#[cfg(target_os = "macos")]
pub(crate) mod validation;

use std::{ffi::OsString, process::ExitCode};

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
use std::{io, path::Path, process::Command};

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
use crate::release_resources::platform;

pub(super) fn run(args: &[OsString]) -> ExitCode {
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    if let Err(error) = stage(args) {
        eprintln!("cargo xtask stage-release-notices: {error}");
        return ExitCode::FAILURE;
    }
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    return ExitCode::SUCCESS;
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    {
        let _ = args;
        eprintln!("Release notice staging requires macOS or GNU/Linux.");
        ExitCode::FAILURE
    }
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
fn stage(args: &[OsString]) -> io::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut generator = Command::new("python3");
    generator
        .arg(workspace.join("packaging/release/stage-notices.py"))
        .current_dir(workspace)
        .env("PYTHONDONTWRITEBYTECODE", "1");
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        return generate(generator.arg("--help"));
    }
    let (pairs, remainder) = args.as_chunks::<2>();
    let usage = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected option/value pairs with one --output path. Run cargo xtask stage-release-notices --help",
        )
    };
    if !remainder.is_empty() {
        return Err(usage());
    }
    let mut output = None;
    for [flag, value] in pairs {
        if flag == "--output" {
            if output.replace(value).is_some() || value.is_empty() {
                return Err(usage());
            }
        } else {
            generator.arg(flag).arg(value);
        }
    }
    let output = Path::new(output.ok_or_else(usage)?);
    if !output.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supply an absolute output path",
        ));
    }
    let name = output.file_name().ok_or_else(usage)?;
    let parent_path = output.parent().ok_or_else(usage)?;
    let parent = platform::directory(parent_path)?;
    platform::owned_output_parent(&parent)?;
    platform::require_absent(&parent, name)?;
    platform::with_current_directory(&parent, || {
        let scratch = platform::Scratch::create()?;
        let publish = std::path::absolute(scratch.path().join("publish"))?;
        generate(generator.arg("--output").arg(&publish))?;
        let staging = platform::directory(scratch.path())?;
        platform::publish_release(&staging, &parent, parent_path, name)?;
        staging.sync_all().and_then(|()| parent.sync_all())
    })?;
    println!("Staged release notices in {}", output.display());
    Ok(())
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
fn generate(command: &mut Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "notice generator returned {status}"
        )))
    }
}
