//! Manage the packaged LaunchAgent through the signed sibling daemon.

use std::{io, process::Stdio, time::Duration};

use tokio::{io::AsyncReadExt as _, process::Command, time::timeout};

/// An explicit operation on the per-user macOS login item.
#[derive(Clone, Copy)]
pub enum LoginAction {
    /// Read the current setting.
    Status,
    /// Register the packaged daemon.
    Enable,
    /// Unregister the packaged daemon.
    Disable,
}

/// The registration state reported by macOS.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum LoginStatus {
    /// The item starts at login.
    Enabled,
    /// The item has no active registration.
    Disabled,
    /// The owner must approve the item in System Settings.
    RequiresApproval,
}

/// Query or change the packaged daemon's start-at-login setting.
pub async fn configure_login(action: LoginAction) -> io::Result<LoginStatus> {
    let mut child = Command::new(super::platform::signed_daemon().await?)
        .args([
            "login",
            match action {
                LoginAction::Status => "status",
                LoginAction::Enable => "enable",
                LoginAction::Disable => "disable",
            },
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    timeout(Duration::from_secs(10), async {
        let mut output = Vec::new();
        let mut stdout = child.stdout.take().ok_or_else(invalid_response)?;
        (&mut stdout).take(65).read_to_end(&mut output).await?;
        if output.len() > 64 || !child.wait().await?.success() {
            return Err(invalid_response());
        }
        match std::str::from_utf8(&output).ok().map(str::trim) {
            Some("enabled") => Ok(LoginStatus::Enabled),
            Some("disabled") => Ok(LoginStatus::Disabled),
            Some("requires-approval") => Ok(LoginStatus::RequiresApproval),
            _ => Err(invalid_response()),
        }
    })
    .await?
}

fn invalid_response() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid response from the login item helper",
    )
}
