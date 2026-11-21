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
    for command in ["guest", "bundle", "sign", "package", "bench"] {
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

#[test]
fn guest_help_lists_build_stages() {
    let output = run(&["guest", "--help"]);
    assert!(output.status.success());
    let help = str::from_utf8(&output.stdout).expect("UTF-8 help");
    for stage in ["kernel", "rootfs", "image", "all"] {
        assert!(help.contains(stage), "{help}");
    }
}

#[test]
fn guest_rejects_unknown_build_stages() {
    let output = run(&["guest", "unknown"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn fetch_upstream_help_describes_staged_output() {
    let output = run(&["fetch-upstream", "--help"]);
    assert!(output.status.success());
    let help = str::from_utf8(&output.stdout).expect("UTF-8 help");
    assert!(help.contains("target/upstream/"), "{help}");
}

#[test]
fn fetch_upstream_rejects_unknown_options() {
    let output = run(&["fetch-upstream", "--unknown"]);
    assert_eq!(output.status.code(), Some(2));
}
