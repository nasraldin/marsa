//! Exercise byte preservation, conflict detection and private output.

use std::{fs, os::unix::fs::symlink};

use super::*;

#[test]
fn inserts_only_the_store_field_and_keeps_an_existing_policy_byte_for_byte() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".docker");
    fs::create_dir(&path).unwrap();
    let configuration = path.join(CONFIG);
    let uid = nix::unistd::geteuid().as_raw();
    let original = br#" { "currentContext": "local-dev", "auths": {} } "#;
    fs::write(&configuration, original).unwrap();

    assert!(enable(&path, uid).unwrap());
    let result = fs::read(&configuration).unwrap();
    assert_eq!(
        result,
        b" {\n  \"credsStore\": \"osxkeychain\", \"currentContext\": \"local-dev\", \"auths\": {} } "
    );
    assert_eq!(fs::metadata(&configuration).unwrap().mode() & 0o777, 0o600);
    assert!(!enable(&path, uid).unwrap());
    assert_eq!(fs::read(&configuration).unwrap(), result);

    for configured in [
        br#"{"credsStore":"desktop","currentContext":"desktop-linux"}"#.as_slice(),
        br#"{"credHelpers":{"demo.invalid":"pass"}}"#,
        br#"{"CREDSSTORE":"desktop"}"#,
    ] {
        fs::write(&configuration, configured).unwrap();
        assert!(!enable(&path, uid).unwrap());
        assert_eq!(fs::read(&configuration).unwrap(), configured);
    }
}

#[test]
fn leaves_existing_legacy_credentials_and_redirects_untouched() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".docker");
    fs::create_dir(&path).unwrap();
    let uid = nix::unistd::geteuid().as_raw();
    let configuration = path.join(CONFIG);
    let original = br#"{"auths":{"demo.invalid":{"auth":"fixture-value"}}}"#;
    fs::write(&configuration, original).unwrap();
    assert!(matches!(
        enable(&path, uid),
        Err(Error::ExistingCredentials)
    ));
    assert_eq!(fs::read(&configuration).unwrap(), original);

    let elsewhere = root.path().join("other.json");
    fs::rename(&configuration, &elsewhere).unwrap();
    symlink(&elsewhere, &configuration).unwrap();
    assert!(matches!(enable(&path, uid), Err(Error::Unsafe)));
    assert_eq!(fs::read(&elsewhere).unwrap(), original);
}

#[test]
fn creates_a_private_new_config_and_refuses_stale_publication() {
    let root = tempfile::tempdir().unwrap();
    let location = root.path().join("new-config");
    let uid = nix::unistd::geteuid().as_raw();
    assert!(enable(&location, uid).unwrap());
    let config = location.join(CONFIG);
    let directory = File::open(location).unwrap();
    let existing = read(&directory, uid).unwrap();
    fs::write(&config, b"{ \"currentContext\": \"other\" }").unwrap();
    assert!(matches!(
        publish(&directory, uid, existing, b"{}"),
        Err(Error::Changed)
    ));
    assert_eq!(
        fs::read(&config).unwrap(),
        b"{ \"currentContext\": \"other\" }"
    );
    assert_eq!(fs::read_dir(config.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn rejects_ambiguous_top_level_fields() {
    let raw = br#"{"auths":{"demo.invalid":{"auth":"fixture-value"}},"AUTHs":{}}"#;
    assert!(matches!(add_setting(raw), Err(Error::Malformed)));
}
