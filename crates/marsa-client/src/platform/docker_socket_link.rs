//! macOS administrator consent for one fixed compatibility link.

use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    process::Stdio,
};

use tokio::{fs, process::Command};

use crate::docker_socket_path;

const LINK: &str = "/var/run/docker.sock";
const LINK_SCRIPT: &str = include_str!("docker_socket_link.sh");
const ADMIN_SCRIPT: &str = r#"
on run argv
    set arguments to ""
    repeat with value in argv
        set arguments to arguments & " " & quoted form of (contents of value)
    end repeat
    try
        do shell script ("/bin/sh -c" & arguments) with administrator privileges
        return "completed"
    on error number -128
        return "cancelled"
    end try
end run
"#;

/// Explicit Settings actions for the legacy system Docker socket.
#[derive(Debug, Clone, Copy)]
pub enum LinkAction {
    /// Create the link to this user's private Marsa socket.
    Install,
    /// Remove a link that points to this user's private Marsa socket.
    Remove,
}

/// Result of a legacy socket change.
#[derive(Debug, Clone, Copy)]
pub enum LinkOutcome {
    /// The public path is in the requested state.
    Completed,
    /// The user dismissed the macOS authorization prompt.
    Cancelled,
    /// The path belongs to another engine or has another filesystem type.
    Occupied,
    /// The owner's private Marsa socket is unavailable for installation.
    TargetUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkState {
    Absent,
    Installed,
    Occupied,
}

/// Request administrator approval for a single legacy Docker link change.
///
/// This entry point is intended for a confirmed user action in Marsa Settings.
pub async fn configure(action: LinkAction) -> io::Result<LinkOutcome> {
    if nix::unistd::geteuid().is_root() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let target = docker_socket_path().map_err(io::Error::other)?;
    let link = Path::new(LINK);
    if target.to_str().is_none() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let before = inspect_async(link, &target).await?;
    match (action, before) {
        (_, LinkState::Occupied) => return Ok(LinkOutcome::Occupied),
        (LinkAction::Install, LinkState::Installed) | (LinkAction::Remove, LinkState::Absent) => {
            return Ok(LinkOutcome::Completed);
        }
        _ => {}
    }
    if matches!(action, LinkAction::Install) && !private_socket(&target).await? {
        return Ok(LinkOutcome::TargetUnavailable);
    }

    let directory = fs::metadata("/private/var/run").await?;
    if !directory.is_dir() || directory.uid() != 0 || directory.mode() & 0o002 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let action_name = match action {
        LinkAction::Install => "install",
        LinkAction::Remove => "remove",
    };
    let output = admin_command(ADMIN_SCRIPT, action_name, link, &target)
        .output()
        .await?;
    if output.status.success() && output.stdout == b"cancelled\n" {
        return Ok(LinkOutcome::Cancelled);
    }

    let after = inspect_async(link, &target).await?;
    match (action, after) {
        (_, LinkState::Occupied) => Ok(LinkOutcome::Occupied),
        (LinkAction::Install, LinkState::Installed) | (LinkAction::Remove, LinkState::Absent) => {
            Ok(LinkOutcome::Completed)
        }
        _ => Err(io::Error::other(
            "macOS did not apply the Docker socket link change",
        )),
    }
}

fn admin_command(script: &str, action: &str, link: &Path, target: &Path) -> Command {
    let mut command = Command::new("/usr/bin/osascript");
    command
        .args([
            "-e",
            script,
            "--",
            LINK_SCRIPT,
            "marsa-docker-socket",
            action,
        ])
        .arg(link)
        .arg(target)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn inspect_async(link: &Path, target: &Path) -> io::Result<LinkState> {
    let link = link.to_path_buf();
    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || inspect(&link, &target))
        .await
        .map_err(io::Error::other)?
}

fn inspect(link: &Path, target: &Path) -> io::Result<LinkState> {
    match std::fs::read_link(link) {
        Ok(found) if found.as_os_str() == target.as_os_str() => Ok(LinkState::Installed),
        Ok(_) => Ok(LinkState::Occupied),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LinkState::Absent),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(LinkState::Occupied),
        Err(error) => Err(error),
    }
}

async fn private_socket(target: &Path) -> io::Result<bool> {
    let parent = target.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let directory = match fs::symlink_metadata(parent).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let socket = match fs::symlink_metadata(target).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let uid = nix::unistd::geteuid().as_raw();
    Ok(directory.is_dir()
        && socket.file_type().is_socket()
        && [directory, socket]
            .iter()
            .all(|entry| entry.uid() == uid && entry.mode() & 0o077 == 0))
}

#[cfg(test)]
#[path = "docker_socket_link_tests.rs"]
mod tests;
