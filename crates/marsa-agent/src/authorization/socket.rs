//! Verify the fixed guest endpoint used by Docker-in-Docker socket clients.

use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};

pub(crate) const GUEST_ENGINE_SOCKET: &str = "/var/run/docker.sock";

#[cfg(target_os = "linux")]
pub(crate) async fn is_guest_engine_socket() -> bool {
    tokio::task::spawn_blocking(|| verify(Path::new("/"), 0))
        .await
        .unwrap_or_default()
}

fn verify(root: &Path, owner: u32) -> bool {
    let run = root.join("run");
    let var = root.join("var");
    let alias = var.join("run");

    // Both parent directories are protected. Accept only the symlink provided
    // by the pinned guest rootfs and require a real, non-public Unix socket.
    [&run, &var].into_iter().all(|directory| {
        fs::symlink_metadata(directory).is_ok_and(|metadata| {
            metadata.is_dir() && metadata.uid() == owner && metadata.mode() & 0o022 == 0
        })
    }) && fs::symlink_metadata(&alias)
        .is_ok_and(|metadata| metadata.is_symlink() && metadata.uid() == owner)
        && fs::read_link(alias).is_ok_and(|target| target == Path::new("../run"))
        && fs::symlink_metadata(run.join("docker.sock")).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.uid() == owner
                && metadata.mode() & 0o002 == 0
        })
}

#[cfg(test)]
mod tests {
    use std::os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    };

    use super::*;

    #[test]
    fn verifies_fixture_socket_and_rejects_wrong_type_permissions_owner_and_alias() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let owner = root.metadata().unwrap().uid();
        fs::create_dir(root.join("run")).unwrap();
        fs::create_dir(root.join("var")).unwrap();
        symlink("../run", root.join("var/run")).unwrap();
        let path = root.join("run/docker.sock");

        fs::File::create(&path).unwrap();
        assert!(!verify(root, owner));
        fs::remove_file(&path).unwrap();

        let _docker = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o660)).unwrap();
        assert!(verify(root, owner));
        assert!(!verify(root, owner.wrapping_add(1)));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(!verify(root, owner));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o660)).unwrap();

        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o775)).unwrap();
        assert!(!verify(root, owner));
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o755)).unwrap();

        fs::remove_file(root.join("var/run")).unwrap();
        symlink("../other", root.join("var/run")).unwrap();
        assert!(!verify(root, owner));
    }
}
