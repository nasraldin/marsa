//! Keep login-item commands explicit and separate from daemon startup.

#![cfg(target_os = "macos")]

use std::{
    process::{Command, Output, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use tempfile::TempDir;

#[test]
fn login_status_requires_a_containing_app_bundle() {
    let output = login_command(&["login", "status"], "en-US");
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("inside a signed Marsa app bundle")
    );
}

#[test]
fn invalid_login_arguments_are_rejected_before_registration() {
    let output = login_command(&["login", "enable", "unexpected"], "en-US");
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("login status|enable|disable")
    );
}

#[test]
fn login_diagnostics_support_arabic() {
    let output = login_command(&["login", "status"], "ar_AE.UTF-8");
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("حزمة تطبيق مرسى الموقّعة")
    );
}

fn login_command(args: &[&str], locale: &str) -> Output {
    let home = TempDir::new_in("/tmp").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_marsad"))
        .args(args)
        .env("HOME", home.path())
        .env("LC_ALL", locale)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().unwrap().is_some() {
            assert!(!home.path().join(".marsa").exists());
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("login command attempted to start the daemon");
        }
        sleep(Duration::from_millis(20));
    }
}
