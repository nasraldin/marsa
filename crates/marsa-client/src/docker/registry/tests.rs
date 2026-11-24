use super::*;

#[test]
fn registry_authority_does_not_accept_urls_aliases_or_hidden_namespaces() {
    assert_eq!(
        RegistryAddress::parse("hub.docker.com").unwrap().as_str(),
        "hub.docker.com"
    );
    assert_eq!(
        RegistryAddress::parse("registry.example:6443")
            .unwrap()
            .as_str(),
        "registry.example:6443"
    );
    assert_eq!(
        RegistryAddress::parse("index.docker.io").unwrap(),
        RegistryAddress::docker_hub()
    );
    for value in [
        "",
        "registry",
        "localhost",
        "https://registry.example",
        "http://registry.example",
        "other@registry.example",
        "registry.example/v2",
        "REGISTRY.example",
        "registry.example.",
        "registry.example:0",
        "registry.example:65536",
        "registry.example:0443",
        "registries.marsa.invalid",
        "192.0.2.1",
        "registry-1.docker.io",
    ] {
        assert!(RegistryAddress::parse(value).is_err(), "{value}");
    }
}

#[test]
fn managed_entries_are_namespaced_and_only_latest_metadata_is_listed() {
    let registry = RegistryAddress::parse("registry.example:5443").unwrap();
    let profile = Profile::new(Path::new("/owned/app-data"));
    let other_profile = Profile::new(Path::new("/other/app-data"));
    let older = Key::new(registry.clone(), 1);
    let latest = Key::new(registry.clone(), 2);
    let values = BTreeMap::from([
        ("https://registry.example:5443".into(), "cli-user".into()),
        (
            "https://registries.marsa.invalid/v1/malformed/1".into(),
            "invalid".into(),
        ),
        (profile.url(&older), "old-user".into()),
        (profile.url(&latest), "new-user".into()),
        (other_profile.url(&latest), "other-user".into()),
    ]);
    let listed = sorted_keys(values, &profile);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].key, latest);
    assert_eq!(listed[0].username, "new-user");
    assert_eq!(listed[1].key, older);
    assert_eq!(profile.parse(&profile.url(&latest)), Some(latest));
}

#[test]
fn registry_basic_check_validates_authority_and_rejects_redirects() {
    let registry = RegistryAddress::parse("registry.example").unwrap();
    assert_eq!(
        probe::registry_url(&registry),
        "https://registry.example/v2/"
    );
    assert_eq!(
        probe::registry_url(&RegistryAddress::docker_hub()),
        "https://registry-1.docker.io/v2/"
    );
    assert!(probe::basic_challenge(
        "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"Owners\"\r\n\r\n"
    ));
    for response in [
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: https://other.example/v2/\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"http://other.example/token\"\r\n\r\n",
        "HTTP/1.1 200 OK\r\n\r\n",
    ] {
        assert!(!probe::basic_challenge(response), "{response}");
    }
}

#[tokio::test]
async fn failed_replacement_preserves_active_credential_and_never_mutates_cli_key() {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
    };

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("app-state");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let old_marker = fixture.path().join("old");
    let new_marker = fixture.path().join("new");
    let helper_path = fixture.path().join("helper");
    fs::write(&old_marker, "").unwrap();

    let address = RegistryAddress::parse("private.example:5443").unwrap();
    let profile = Profile::new(&root);
    let old_key = profile.url(&Key::new(address.clone(), 1));
    let new_key = profile.url(&Key::new(address.clone(), 2));
    fs::write(
        &helper_path,
        format!(
            r#"#!/bin/sh
case "$1" in
  list)
    printf '{{"https://private.example:5443":"cli-user"'
    test ! -f '{old_marker}' || printf ',"{old_key}":"old-user"'
    test ! -f '{new_marker}' || printf ',"{new_key}":"new-user"'
    printf '}}'
    ;;
  store)
    test "$#" -eq 1 || exit 4
    payload=$(cat)
    case "$payload" in
      *reject-test-credential*) exit 5 ;;
      *{new_key}*) touch '{new_marker}' ;;
      *) exit 6 ;;
    esac
    ;;
  erase)
    case "$(cat)" in
      '{old_key}') rm '{old_marker}' ;;
      '{new_key}') rm '{new_marker}' ;;
      *) exit 7 ;;
    esac
    ;;
  *) exit 8 ;;
esac
"#,
            old_marker = old_marker.display(),
            new_marker = new_marker.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o700)).unwrap();
    let store = RegistryStore {
        helper: helper_path,
        directory: root,
        profile,
    };

    let previous = store.list().await.unwrap().remove(0);
    let rejected =
        RegistryCredentials::new("new-user".into(), "reject-test-credential".into()).unwrap();
    assert!(matches!(
        store
            .save(address.clone(), rejected, Some(previous.clone()))
            .await,
        Err(RegistryError::Keychain)
    ));
    assert_eq!(store.list().await.unwrap(), vec![previous.clone()]);
    assert!(old_marker.exists());
    assert!(!new_marker.exists());

    let replacement =
        RegistryCredentials::new("new-user".into(), "test-credential".into()).unwrap();
    store
        .save(address, replacement, Some(previous.clone()))
        .await
        .unwrap();
    assert!(!old_marker.exists());
    assert!(new_marker.exists());
    assert!(matches!(
        store.remove(previous).await,
        Err(RegistryError::Changed)
    ));
    let current = store.list().await.unwrap().remove(0);
    assert_eq!(current.username(), "new-user");
    store.remove(current).await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
    assert!(!new_marker.exists());
}
