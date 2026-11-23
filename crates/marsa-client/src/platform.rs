//! Authorize local Unix sockets and spawn the daemon attached to the current app.

use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
};

mod docker_transport;
mod lima;
pub(crate) use lima::{check_lima_target, trusted_lima_binary};
pub(crate) mod registry_https;
pub(crate) use docker_transport::{
    check_user_docker_directory, docker_ssh_config, lock_docker_state, open_docker_ssh,
    read_user_docker_file, replace_docker_state,
};

use nix::unistd::geteuid;
use tokio::{fs, net::UnixStream, process::Command};

pub(super) async fn open_socket(path: PathBuf) -> io::Result<UnixStream> {
    open_local_socket(path, 0o077, None).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SocketIdentity(u64, u64);

pub(super) async fn socket_identity(path: &Path) -> io::Result<SocketIdentity> {
    let metadata = fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_socket() {
        return Err(unsafe_endpoint());
    }
    Ok(SocketIdentity(metadata.dev(), metadata.ino()))
}

pub(super) async fn open_external_socket(
    path: PathBuf,
    expected: SocketIdentity,
) -> io::Result<UnixStream> {
    open_local_socket(path, 0o022, Some(expected)).await
}

async fn open_local_socket(
    path: PathBuf,
    forbidden_parent_modes: u32,
    expected: Option<SocketIdentity>,
) -> io::Result<UnixStream> {
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
    if expected.is_some_and(|expected| expected != SocketIdentity(metadata.dev(), metadata.ino())) {
        return Err(unsafe_endpoint());
    }
    let socket = UnixStream::connect(&path).await?;
    if let Some(expected) = expected
        && socket_identity(&path).await? != expected
    {
        return Err(unsafe_endpoint());
    }
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

#[cfg(target_os = "macos")]
pub(super) async fn system_git() -> io::Result<PathBuf> {
    if geteuid().is_root() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let path = PathBuf::from("/usr/bin/git");
    let binary = fs::symlink_metadata(&path).await?;
    if !binary.is_file()
        || binary.uid() != 0
        || binary.mode() & 0o111 == 0
        || binary.mode() & 0o6022 != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(path)
}

pub(super) async fn bundled_compose() -> io::Result<PathBuf> {
    bundled_program("Contents/Resources/bin/cli-plugins/docker-compose").await
}

pub(super) async fn bundled_registry_helper() -> io::Result<PathBuf> {
    bundled_program("Contents/Resources/bin/docker-credential-osxkeychain").await
}

async fn bundled_program(_relative: &str) -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let executable = fs::canonicalize(std::env::current_exe()?).await?;
        let bundle = executable
            .parent()
            .and_then(app_bundle)
            .ok_or_else(unsafe_daemon)?;
        let binary = bundle.join(_relative);
        let metadata = fs::symlink_metadata(&binary).await?;
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 || metadata.mode() & 0o6022 != 0 {
            return Err(unsafe_daemon());
        }
        verify_signature(bundle).await?;
        verify_signature(&binary).await?;
        Ok(binary)
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
