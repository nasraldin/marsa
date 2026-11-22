//! Authorize local Unix sockets and spawn the daemon attached to the current app.

use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
};

use nix::unistd::geteuid;
use tokio::{fs, net::UnixStream, process::Command};

pub(super) async fn open_socket(path: PathBuf) -> io::Result<UnixStream> {
    open_local_socket(path, 0o077).await
}

pub(super) async fn open_inventory_socket(path: PathBuf) -> io::Result<UnixStream> {
    open_local_socket(path, 0o022).await
}

async fn open_local_socket(path: PathBuf, forbidden_parent_modes: u32) -> io::Result<UnixStream> {
    let parent = path
        .parent()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected an absolute daemon socket path",
            )
        })?;
    let uid = geteuid().as_raw();
    let directory = fs::symlink_metadata(parent).await?;
    if !directory.is_dir()
        || directory.uid() != uid
        || directory.mode() & forbidden_parent_modes != 0
    {
        return Err(unsafe_endpoint());
    }
    let metadata = fs::symlink_metadata(&path).await?;
    if !metadata.file_type().is_socket() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(unsafe_endpoint());
    }
    let socket = UnixStream::connect(path).await?;
    if socket.peer_cred()?.uid() != uid {
        return Err(unsafe_endpoint());
    }
    Ok(socket)
}

fn unsafe_endpoint() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "the daemon socket or its Unix peer does not belong to this user",
    )
}

pub(super) async fn launch_daemon() -> io::Result<()> {
    let executable = fs::canonicalize(std::env::current_exe()?).await?;
    let daemon = daemon_path(&executable).await?;
    let mut child = Command::new(daemon)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Let the daemon outlive the calling CLI. Reap it if it exits while the
    // client is alive, including when another launcher won the daemon lock.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) async fn signed_daemon() -> io::Result<PathBuf> {
    let executable = fs::canonicalize(std::env::current_exe()?).await?;
    executable
        .parent()
        .and_then(app_bundle)
        .ok_or_else(unsafe_daemon)?;
    daemon_path(&executable).await
}

async fn daemon_path(executable: &Path) -> io::Result<PathBuf> {
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("could not locate the Marsa executable directory"))?;
    let daemon = directory.join("marsad");
    let metadata = fs::symlink_metadata(&daemon).await?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 || metadata.mode() & 0o6022 != 0 {
        return Err(unsafe_daemon());
    }
    #[cfg(target_os = "macos")]
    if let Some(bundle) = app_bundle(directory) {
        verify_signature(bundle).await?;
        verify_signature(&daemon).await?;
        return Ok(daemon);
    }
    if cfg!(debug_assertions) {
        let directory = fs::metadata(directory).await?;
        if directory.uid() == geteuid().as_raw() && directory.mode() & 0o022 == 0 {
            return Ok(daemon);
        }
    }
    Err(unsafe_daemon())
}

pub(super) async fn bundled_compose() -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let executable = fs::canonicalize(std::env::current_exe()?).await?;
        let bundle = executable
            .parent()
            .and_then(app_bundle)
            .ok_or_else(unsafe_daemon)?;
        let compose = bundle.join("Contents/Resources/bin/cli-plugins/docker-compose");
        let metadata = fs::symlink_metadata(&compose).await?;
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 || metadata.mode() & 0o6022 != 0 {
            return Err(unsafe_daemon());
        }
        verify_signature(bundle).await?;
        verify_signature(&compose).await?;
        Ok(compose)
    }
    #[cfg(not(target_os = "macos"))]
    Err(io::ErrorKind::Unsupported.into())
}

fn unsafe_daemon() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "expected the executable marsad from the signed Marsa app bundle",
    )
}

#[cfg(target_os = "macos")]
fn app_bundle(directory: &Path) -> Option<&Path> {
    let contents = directory.parent()?;
    let bundle = contents.parent()?;
    (directory.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then_some(bundle)
}

#[cfg(target_os = "macos")]
async fn verify_signature(path: &Path) -> io::Result<()> {
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(path)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if !status.success() {
        return Err(unsafe_daemon());
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bundled_launch_requires_valid_app_and_daemon_signatures() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let bundle = directory.path().join("Marsa.app");
        let contents = bundle.join("Contents");
        let programs = contents.join("MacOS");
        fs::create_dir_all(&programs).await.unwrap();
        let client = programs.join("marsa");
        let daemon = programs.join("marsad");
        fs::copy("/usr/bin/true", &client).await.unwrap();
        fs::copy("/usr/bin/true", &daemon).await.unwrap();
        fs::write(
            contents.join("Info.plist"),
            br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.github.nasraldin.marsa.fixture</string>
  <key>CFBundleExecutable</key><string>marsa</string>
  <key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#,
        )
        .await
        .unwrap();
        for path in [&daemon, &bundle] {
            assert!(
                Command::new("/usr/bin/codesign")
                    .args(["--force", "--sign", "-", "--timestamp=none"])
                    .arg(path)
                    .stderr(Stdio::null())
                    .status()
                    .await
                    .unwrap()
                    .success()
            );
        }
        assert_eq!(daemon_path(&client).await.unwrap(), daemon);
        fs::write(contents.join("Info.plist"), b"invalid app resource seal")
            .await
            .unwrap();
        assert_eq!(
            daemon_path(&client).await.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
        );
    }
}
