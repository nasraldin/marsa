//! Verify local ownership and file types before invoking the installed Lima CLI.

use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use nix::unistd::geteuid;
use tokio::fs;

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

#[cfg(test)]
mod tests {
    use super::*;

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
