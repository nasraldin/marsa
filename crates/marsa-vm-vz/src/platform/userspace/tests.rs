use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use marsa_vm::{VmEvent, VmState};
use tokio::time::timeout;
use tokio_stream::StreamExt;

use super::Transport;
use crate::platform::native::Controller;

fn write_helper(directory: &Path, body: &str) -> std::path::PathBuf {
    let path = directory.join("helper");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test]
async fn absent_or_untrusted_executable_fails_before_start() {
    let scratch = tempfile::tempdir().unwrap();
    let missing = scratch.path().join("missing");
    assert!(Transport::new(&missing).is_err());
    let helper = write_helper(scratch.path(), "exit 0");
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Transport::new(&helper).is_err());
}

#[tokio::test]
async fn helper_must_signal_readiness() {
    let scratch = tempfile::tempdir().unwrap();
    let helper = write_helper(scratch.path(), "printf 'WRONG\\n'");
    let (mut transport, _socket) = Transport::new(&helper).unwrap();
    let error = transport.start(&Controller::new()).await.unwrap_err();
    assert!(error.to_string().contains("readiness"), "{error}");
}

#[tokio::test]
async fn a_silent_helper_has_a_bounded_startup() {
    let scratch = tempfile::tempdir().unwrap();
    let helper = write_helper(scratch.path(), "exec /bin/sleep 60");
    let (mut transport, _socket) = Transport::new(&helper).unwrap();
    let error = transport.start(&Controller::new()).await.unwrap_err();
    assert!(matches!(
        error,
        marsa_vm::VmError::Timeout {
            operation: super::START,
            limit: super::START_LIMIT
        }
    ));
    assert!(transport.session.is_none());
}

#[tokio::test]
async fn helper_ends_on_stop_and_can_start_again() {
    let scratch = tempfile::tempdir().unwrap();
    let helper = write_helper(scratch.path(), "printf 'READY\\n'\nexec /bin/sleep 60");
    let (mut transport, _socket) = Transport::new(&helper).unwrap();
    let controller = Controller::new();
    for _ in 0..2 {
        transport.start(&controller).await.unwrap();
        let session = transport.session.as_ref().unwrap();
        assert!(!session.worker.is_finished());
        transport.stop().await;
        assert!(transport.session.is_none());
    }
}

#[tokio::test]
async fn unexpected_helper_exit_is_diagnosed_without_ending_guest() {
    let scratch = tempfile::tempdir().unwrap();
    let helper = write_helper(scratch.path(), "printf 'READY\\n'\n/bin/sleep 0.2\nexit 77");
    let (mut transport, _socket) = Transport::new(&helper).unwrap();
    let controller = Controller::new();
    let mut events = controller.lifecycle.events();
    controller
        .call("test state", |controller| {
            controller.lifecycle.transition(VmState::Running);
            Ok(())
        })
        .await
        .unwrap();
    let _ = events.next().await;
    transport.start(&controller).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(2), events.next()).await.unwrap(),
        Some(VmEvent::NetworkUnavailable { detail }) if detail.contains("77")
    ));
    assert_eq!(controller.lifecycle.current(), VmState::Running);
    transport.stop().await;
}
