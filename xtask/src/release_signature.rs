//! Validate the release's Apple code signature before submitting for notarization.

use std::{ffi::OsString, io, path::Path, process::ExitCode};

#[cfg(target_os = "macos")]
mod platform;

const USAGE: &str = "Usage: cargo xtask verify-signature --team-id TEAM_ID PATH_TO_MARSA_APP";

pub(super) fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let [flag, team, bundle] = args else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let Some(team) = team.to_str().filter(|value| valid_team_id(value)) else {
        eprintln!("{USAGE}\nUse a ten-character uppercase Apple Team ID.");
        return ExitCode::from(2);
    };
    if flag != "--team-id" || bundle.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    match verify(Path::new(bundle), team) {
        Ok(count) => {
            println!(
                "Verified the Developer ID signature and release policy for {count} Mach-O files."
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cargo xtask verify-signature: {error}");
            ExitCode::FAILURE
        }
    }
}

fn valid_team_id(value: &str) -> bool {
    value.len() == 10
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

#[cfg(target_os = "macos")]
fn verify(bundle: &Path, team: &str) -> io::Result<usize> {
    platform::verify(bundle, team)
}

#[cfg(not(target_os = "macos"))]
fn verify(_: &Path, _: &str) -> io::Result<usize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Apple release signature verification requires macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::valid_team_id;

    #[test]
    fn team_id_cannot_change_the_apple_requirement_expression() {
        assert!(valid_team_id("ABCD123456"));
        for value in [
            "",
            "short",
            "abcd123456",
            "ABCD12345\" ",
            "ABCDEFGHIJK",
            "ÅBCD12345",
        ] {
            assert!(!valid_team_id(value), "{value}");
        }
    }
}
