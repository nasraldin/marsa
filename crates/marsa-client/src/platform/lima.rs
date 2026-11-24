//! Verify local ownership and file types before invoking the installed Lima CLI.

use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use nix::unistd::geteuid;
use tokio::{fs, process::Command};

use super::check_user_docker_directory;

pub(crate) async fn check_lima_target(root: &Path, name: &str, socket: &Path) -> io::Result<()> {
    if fs::canonicalize(root).await?.as_os_str() != root.as_os_str() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    check_user_docker_directory(root).await?;
    let instance = root.join(name);
    check_user_docker_directory(&instance).await?;
    let config = fs::symlink_metadata(instance.join("lima.yaml")).await?;
    let uid = geteuid().as_raw();
    if !config.is_file() || config.uid() != uid || config.mode() & 0o022 != 0 || config.nlink() != 1
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }

    match check_user_docker_directory(&instance.join("sock")).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let metadata = match fs::symlink_metadata(socket).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

pub(crate) async fn trusted_lima_binary(candidate: &Path) -> io::Result<PathBuf> {
    let path = fs::canonicalize(candidate).await?;
    let metadata = fs::symlink_metadata(&path).await?;
    if !metadata.is_file()
        || ![0, geteuid().as_raw()].contains(&metadata.uid())
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o6022 != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    for directory in path.ancestors().skip(1) {
        let directory = fs::symlink_metadata(directory).await?;
        if !directory.is_dir()
            || ![0, geteuid().as_raw()].contains(&directory.uid())
            || directory.mode() & 0o002 != 0
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
pub(crate) fn lima_lifecycle_command(binary: &Path, instance: &Path) -> io::Result<Command> {
    if geteuid().is_root() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    // Lima 2.3's network and block-device helpers use host sudo. Its automatic
    // start path can also delegate to launchd, outside the inherited profile.
    // Let ordinary Lima and its background host agent run, while denying those
    // privileged or delegated paths even if the user has passwordless grants.
    // A deleted instance must not be replaced by `limactl start`'s default
    // template fallback. Limit create and unlink of the actual directory entry,
    // while letting Lima update its own files inside an existing instance.
    const POLICY: &str = r#"(version 1)
        (allow default)
        (deny file-write-create file-write-unlink
            (literal (param "MARSA_LIMA_INSTANCE")))
        (deny process-exec
            (literal "/usr/bin/sudo")
            (literal "/usr/bin/su")
            (literal "/bin/launchctl")
            (literal "/usr/bin/osascript"))"#;
    let mut command = Command::new("/usr/bin/sandbox-exec");
    let mut protected = std::ffi::OsString::from("MARSA_LIMA_INSTANCE=");
    protected.push(instance);
    command
        .arg("-D")
        .arg(protected)
        .args(["-p", POLICY])
        .arg(binary)
        .env(
            "PATH",
            "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin",
        );
    Ok(command)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn lima_lifecycle_command(_: &Path, _: &Path) -> io::Result<Command> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn lima_lifecycle_stays_local_and_cannot_invoke_host_sudo() {
        let temp = tempfile::tempdir().unwrap();
        let instance = temp.path().canonicalize().unwrap().join("instance");
        if geteuid().is_root() {
            assert!(lima_lifecycle_command(Path::new("/usr/bin/true"), &instance).is_err());
            return;
        }
        let allowed = lima_lifecycle_command(Path::new("/usr/bin/true"), &instance)
            .unwrap()
            .status()
            .await
            .unwrap();
        assert!(allowed.success());
        // Even a read-only sudo invocation is denied at exec, before any grants
        // or cached credentials can be consulted.
        let denied = lima_lifecycle_command(Path::new("/bin/sh"), &instance)
            .unwrap()
            .args(["-c", "exec /usr/bin/sudo --version"])
            .output()
            .await
            .unwrap();
        assert!(!denied.status.success());
        assert!(denied.stdout.is_empty());

        let create = lima_lifecycle_command(Path::new("/bin/mkdir"), &instance)
            .unwrap()
            .arg(&instance)
            .output()
            .await
            .unwrap();
        assert!(!create.status.success());
        assert!(!instance.exists());
    }

    #[tokio::test]
    async fn stopped_lima_target_requires_a_regular_instance_config() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join(".lima");
        let instance = root.join("docker");
        let config = instance.join("lima.yaml");
        let socket = instance.join("sock/docker.sock");
        fs::create_dir(&root).await.unwrap();
        fs::create_dir(&instance).await.unwrap();
        fs::write(&config, b"vmType: vz").await.unwrap();

        assert!(check_lima_target(&root, "docker", &socket).await.is_ok());

        let hardlink = instance.join("duplicate.yaml");
        fs::hard_link(&config, &hardlink).await.unwrap();
        assert!(check_lima_target(&root, "docker", &socket).await.is_err());

        fs::remove_file(&hardlink).await.unwrap();
        fs::create_dir(instance.join("sock")).await.unwrap();
        fs::write(&socket, b"not a listening socket").await.unwrap();
        assert!(check_lima_target(&root, "docker", &socket).await.is_err());
    }
}
