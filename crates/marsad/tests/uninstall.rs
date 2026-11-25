//! Exercise the packaged uninstall entry point before it can alter owner state.

#![cfg(target_os = "macos")]

use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn uninstall_requires_the_full_confirmation_before_touching_state() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".marsa").join("config.toml");
    fs::create_dir(config.parent().unwrap()).unwrap();
    fs::write(&config, "original fixture").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_marsad"))
        .arg("uninstall")
        .env("HOME", home.path())
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"UNINSTALL\n")
        .unwrap();
    let result = child.wait_with_output().unwrap();

    assert!(!result.status.success());
    let message = String::from_utf8(result.stderr).unwrap();
    assert!(message.contains("UNINSTALL MARSA"), "{message}");
    assert!(message.contains("Uninstall canceled"), "{message}");
    assert_eq!(fs::read_to_string(&config).unwrap(), "original fixture");
}
