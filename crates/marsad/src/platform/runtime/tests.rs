//! Pin disk identity and mutation behavior without starting a virtual machine.

use std::{
    fs::{self, Permissions},
    os::unix::fs::{PermissionsExt, symlink},
};

use super::*;

const GIB: u64 = 1024 * MIB;

#[test]
fn existing_disk_grows_in_place_and_never_shrinks() {
    let (_home, paths, uid) = private_state();
    ensure_disk(&paths, uid, GIB).unwrap();
    let disk = OpenOptions::new()
        .read(true)
        .write(true)
        .open(paths.data_disk())
        .unwrap();
    disk.write_all_at(b"persistent data", 0).unwrap();
    let initial = disk.metadata().unwrap();
    assert_eq!(initial.len(), GIB);
    assert_eq!(initial.mode() & 0o7777, 0o600);

    ensure_disk(&paths, uid, 2 * GIB).unwrap();
    assert_eq!(
        ensure_disk(&paths, uid, GIB).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    let current = fs::metadata(paths.data_disk()).unwrap();
    assert_eq!(current.len(), 2 * GIB);
    assert_eq!(
        (current.dev(), current.ino()),
        (initial.dev(), initial.ino())
    );
    let mut payload = [0; 15];
    disk.read_exact_at(&mut payload, 0).unwrap();
    assert_eq!(&payload, b"persistent data");
}

#[test]
fn disk_preparation_does_not_wait_on_an_existing_lock() {
    let (_home, paths, uid) = private_state();
    ensure_disk(&paths, uid, GIB).unwrap();
    let disk = File::open(paths.data_disk()).unwrap();
    disk.try_lock().unwrap();
    assert_eq!(
        ensure_disk(&paths, uid, 2 * GIB).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(disk.metadata().unwrap().len(), GIB);
}

#[test]
fn disk_preparation_rejects_redirects_and_insecure_existing_files() {
    let (home, paths, uid) = private_state();
    let target = home.path().join("unrelated");
    fs::write(&target, [17; 512]).unwrap();
    fs::set_permissions(&target, Permissions::from_mode(0o600)).unwrap();

    let disk = paths.data_disk();
    symlink(&target, &disk).unwrap();
    assert!(ensure_disk(&paths, uid, GIB).is_err());
    assert!(fs::symlink_metadata(&disk).unwrap().is_symlink());
    assert_eq!(fs::read(&target).unwrap(), [17; 512]);

    fs::remove_file(&disk).unwrap();
    fs::hard_link(&target, &disk).unwrap();
    assert!(ensure_disk(&paths, uid, GIB).is_err());
    assert_eq!(fs::read(&target).unwrap(), [17; 512]);
    assert_eq!(fs::metadata(&disk).unwrap().nlink(), 2);

    fs::remove_file(&disk).unwrap();
    fs::write(&disk, [19; 512]).unwrap();
    fs::set_permissions(&disk, Permissions::from_mode(0o644)).unwrap();
    assert!(ensure_disk(&paths, uid, GIB).is_err());
    assert_eq!(fs::read(&disk).unwrap(), [19; 512]);
    assert_eq!(fs::metadata(&disk).unwrap().mode() & 0o7777, 0o644);
}

#[test]
fn configured_shares_require_the_protected_path_allowlist() {
    let (home, paths, uid) = private_state();
    let mut settings = Settings::default();
    settings.shares.push(home.path().to_owned());
    let package = Packaged {
        bundle: home.path().join("Marsa.app"),
    };
    let error = package.prepare(&paths, uid, &settings).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(!paths.data_disk().exists());
}

#[test]
fn custom_dns_and_proxy_settings_require_the_guest_network_policy() {
    let (home, paths, uid) = private_state();
    let package = Packaged {
        bundle: home.path().join("Marsa.app"),
    };
    for configuration in [
        "[network]\ndns = ['192.0.2.53']",
        "[network.proxy]\nhttp = 'http://proxy.invalid'",
        "[network.proxy]\nhttps = 'https://proxy.invalid'",
        "[network.proxy]\ninject_into_containers = true",
    ] {
        let settings = Settings::from_toml(configuration).unwrap();
        let error = package.prepare(&paths, uid, &settings).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(!paths.data_disk().exists());
    }
}

fn private_state() -> (tempfile::TempDir, MarsaPaths, u32) {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let paths = MarsaPaths::from_home(home.path()).unwrap();
    for path in [paths.root(), &paths.vm_dir()] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
    }
    (home, paths, nix::unistd::geteuid().as_raw())
}
