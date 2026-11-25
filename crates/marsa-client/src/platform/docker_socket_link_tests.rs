use super::*;
use std::{
    fs,
    os::unix::{fs::symlink, net::UnixListener},
    process::Command,
};

fn change(action: &str, link: &Path, target: &Path) -> bool {
    Command::new("/bin/sh")
        .args(["-c", LINK_SCRIPT, "marsa-docker-socket", action])
        .arg(link)
        .arg(target)
        .status()
        .unwrap()
        .success()
}

#[test]
fn one_shot_change_creates_and_removes_only_the_matching_link() {
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let link = fixture.path().join("docker.sock");
    let target = fixture.path().join("owned's socket");
    let unrelated = fixture.path().join("another engine");
    let directory = fixture.path().join("untouched");
    fs::create_dir(&directory).unwrap();
    fs::write(&unrelated, b"keep").unwrap();
    let socket = UnixListener::bind(&target).unwrap();

    assert_eq!(inspect(&link, &target).unwrap(), LinkState::Absent);
    assert!(change("install", &link, &target));
    assert!(change("install", &link, &target));
    assert_eq!(inspect(&link, &target).unwrap(), LinkState::Installed);
    drop(socket);
    fs::remove_file(&target).unwrap();
    assert_eq!(inspect(&link, &target).unwrap(), LinkState::Installed);
    assert!(change("remove", &link, &target));
    assert!(change("remove", &link, &target));

    symlink(&unrelated, &link).unwrap();
    assert_eq!(inspect(&link, &target).unwrap(), LinkState::Occupied);
    assert!(!change("remove", &link, &target));
    assert!(!change("install", &link, &target));
    assert_eq!(fs::read_link(&link).unwrap(), unrelated);
    fs::remove_file(&link).unwrap();

    fs::create_dir(&link).unwrap();
    assert!(!change("remove", &link, &target));
    assert!(!change("install", &link, &target));
    assert_eq!(fs::read_dir(&link).unwrap().count(), 0);
    fs::remove_dir(&link).unwrap();

    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
    assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 2);
}

#[test]
fn link_to_a_similar_path_with_a_trailing_newline_is_foreign() {
    let fixture = tempfile::tempdir().unwrap();
    let link = fixture.path().join("docker.sock");
    let expected = fixture.path().join("owner socket");
    let foreign = fixture.path().join("owner socket\n");
    symlink(&foreign, &link).unwrap();
    assert!(!change("remove", &link, &expected));
    assert_eq!(fs::read_link(&link).unwrap(), foreign);
}
