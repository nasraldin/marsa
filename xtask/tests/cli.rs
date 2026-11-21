//! Verify the public command line and failure behavior of the workspace runner.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .output()
        .expect("launch xtask")
}

#[test]
fn help_lists_all_subcommands() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let help = str::from_utf8(&output.stdout).expect("UTF-8 help");
    for name in [
        "check",
        "test",
        "guest",
        "fetch-upstream",
        "bundle",
        "sign",
        "package",
        "bench",
    ] {
        assert!(help.lines().any(|line| {
            line.trim_start()
                .split_once(' ')
                .is_some_and(|(command, description)| {
                    command == name && !description.trim().is_empty()
                })
        }));
    }
}

#[test]
fn unimplemented_commands_cannot_report_success() {
    for command in [
        "guest",
        "fetch-upstream",
        "bundle",
        "sign",
        "package",
        "bench",
    ] {
        let output = run(&[command]);
        assert!(!output.status.success(), "{command}");
        let error = str::from_utf8(&output.stderr).expect("UTF-8 error");
        assert!(error.contains(command), "{error}");
    }
}

#[test]
fn invalid_invocations_fail() {
    for arguments in [&["unknown"][..], &["check", "--unknown"][..]] {
        assert!(!run(arguments).status.success(), "{arguments:?}");
    }
}
