//! Secure Developer ID delivery and packaging entry points.

use std::{ffi::OsString, io, path::Path, process::ExitCode};

mod platform;
use platform::{package_release, sign_local, sign_release};

const SIGN: &str = "Usage: cargo xtask sign [--local] --app PATH_TO_MARSA_APP --output NEW_DIR\nRelease environment: MARSA_TEAM_ID, MARSA_SIGNING_IDENTITY, MARSA_NOTARY_PROFILE (required for notarization)\n--local stages a Developer ID-signed app for local qualification.";
const PACKAGE: &str = "Usage: cargo xtask package --signed-dir DIR --output NEW_DIR\nRelease environment: MARSA_TEAM_ID";

pub(super) fn sign(args: &[OsString]) -> ExitCode {
    let local = args.iter().filter(|&arg| arg == "--local").count();
    if local > 1 {
        eprintln!("{SIGN}");
        return ExitCode::from(2);
    }
    if local == 1 {
        let args: Vec<_> = args
            .iter()
            .filter(|&arg| arg != "--local")
            .cloned()
            .collect();
        return run(&args, "--app", "sign --local", SIGN, sign_local);
    }
    run(args, "--app", "sign", SIGN, sign_release)
}

pub(super) fn package(args: &[OsString]) -> ExitCode {
    run(args, "--signed-dir", "package", PACKAGE, package_release)
}

fn run(
    args: &[OsString],
    source_flag: &str,
    command: &str,
    usage: &str,
    operate: fn(&Path, &Path) -> io::Result<()>,
) -> ExitCode {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") {
        println!("{usage}");
        return ExitCode::SUCCESS;
    }
    let paths = match args {
        [first, app, second, output] if first == source_flag && second == "--output" => {
            Some((app, output))
        }
        [first, output, second, app] if first == "--output" && second == source_flag => {
            Some((app, output))
        }
        _ => None,
    };
    let Some((source, output)) =
        paths.filter(|(source, output)| !source.is_empty() && !output.is_empty())
    else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    match operate(Path::new(source), Path::new(output)) {
        Ok(()) => {
            println!("Release artifacts written to {:?}", Path::new(output));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cargo xtask {command}: {error}");
            ExitCode::FAILURE
        }
    }
}
