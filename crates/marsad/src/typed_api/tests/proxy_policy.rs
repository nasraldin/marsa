use std::{
    fs::{self, File, Permissions},
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, Settings},
};
use nix::unistd::geteuid;

use super::proxy_policy::BootProxy;

fn fixture() -> (tempfile::TempDir, MarsaPaths) {
    let scratch = tempfile::tempdir().unwrap();
    let paths = MarsaPaths::from_root(scratch.path().to_owned()).unwrap();
    fs::create_dir(paths.state_dir()).unwrap();
    fs::set_permissions(paths.state_dir(), Permissions::from_mode(0o700)).unwrap();
    (scratch, paths)
}

fn store(paths: &MarsaPaths, data: &[u8]) {
    let mut file = File::create(paths.state_dir().join("daemon-proxy.json")).unwrap();
    file.set_permissions(Permissions::from_mode(0o600)).unwrap();
    file.write_all(data).unwrap();
}

#[test]
fn desired_proxy_is_never_used_in_place_of_the_approved_policy() {
    let (_scratch, paths) = fixture();
    let mut desired = Settings::default();
    desired.network.proxy.http = Some("http://draft.example.invalid:8181".into());
    let approved = BootProxy::read(&paths, geteuid().as_raw()).unwrap();
    let effective = approved.effective(desired.clone());
    assert_eq!(effective.network.mode, NetworkMode::Auto);
    assert!(effective.network.proxy.http.is_none());
    assert!(approved.guest_configuration().is_none());

    store(
        &paths,
        br#"{"version":1,"http":"http://approved.example.invalid:8182"}"#,
    );
    let approved = BootProxy::read(&paths, geteuid().as_raw()).unwrap();
    let effective = approved.effective(desired);
    assert_eq!(effective.network.mode, NetworkMode::Userspace);
    assert_eq!(
        effective.network.proxy.http.as_deref(),
        Some("http://approved.example.invalid:8182")
    );
    let config = approved.guest_configuration().unwrap();
    assert!(config.host_aliases.is_empty());
    assert!(config.dns_servers.is_empty());
    assert!(config.search_domains.is_empty());
    assert_eq!(config.proxy.unwrap().http_url, effective.network.proxy.http);
}

#[test]
fn stored_policy_rejects_unsafe_files_and_credentials() {
    let (_scratch, paths) = fixture();
    let path = paths.state_dir().join("daemon-proxy.json");
    let outsider = paths.root().join("arbitrary-file");
    fs::write(&outsider, br#"{"version":1}"#).unwrap();
    symlink(&outsider, &path).unwrap();
    let uid = geteuid().as_raw();
    assert!(BootProxy::read(&paths, uid).is_err());
    fs::remove_file(&path).unwrap();

    store(&paths, br#"{"version":1}"#);
    fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
    assert!(BootProxy::read(&paths, uid).is_err());

    let private_value = "http://test-fixture:secret-password@example.invalid:3128";
    store(
        &paths,
        serde_json::json!({"version": 1, "https": private_value})
            .to_string()
            .as_bytes(),
    );
    let error = BootProxy::read(&paths, uid).unwrap_err();
    assert!(!error.to_string().contains(private_value));
    assert!(!error.to_string().contains("secret-password"));
}

#[test]
fn staged_policy_leaves_the_last_approved_record_until_commit() {
    use marsa_core::settings::ProxySettings;

    let (_scratch, paths) = fixture();
    let uid = geteuid().as_raw();
    store(
        &paths,
        br#"{"version":1,"https":"http://last-approved.invalid:8182"}"#,
    );
    let candidate = BootProxy::from_draft(&ProxySettings {
        https: Some("http://staged.invalid:8183".into()),
        ..Default::default()
    })
    .unwrap();
    let staged = candidate.stage(&paths, uid).unwrap();
    let loaded = BootProxy::read(&paths, uid).unwrap();
    assert_eq!(
        loaded
            .effective(Settings::default())
            .network
            .proxy
            .https
            .as_deref(),
        Some("http://last-approved.invalid:8182")
    );
    drop(staged);
    let rollback = BootProxy::read(&paths, uid).unwrap();
    assert_eq!(rollback, loaded);

    assert!(candidate.stage(&paths, uid).unwrap().commit().unwrap());
    let committed = BootProxy::read(&paths, uid).unwrap();
    assert_eq!(committed, candidate);
}

#[test]
fn unsupported_proxy_injection_and_credentials_are_not_eligible_for_approval() {
    use marsa_core::settings::ProxySettings;

    for invalid in [
        ProxySettings {
            inject_into_containers: true,
            ..Default::default()
        },
        ProxySettings {
            https: Some("http://user:password@proxy.invalid:8183".into()),
            ..Default::default()
        },
    ] {
        let error = BootProxy::from_draft(&invalid).unwrap_err();
        assert!(!error.to_string().contains("password"));
    }
}

#[test]
fn replacing_the_staging_entry_cannot_replace_the_approved_record() {
    use marsa_core::settings::ProxySettings;

    let (_scratch, paths) = fixture();
    let uid = geteuid().as_raw();
    let good = br#"{"version":1,"http":"http://last-approved.invalid:8182"}"#;
    store(&paths, good);
    let candidate = BootProxy::from_draft(&ProxySettings {
        http: Some("http://staged.invalid:8183".into()),
        ..Default::default()
    })
    .unwrap();
    let staged = candidate.stage(&paths, uid).unwrap();
    let entry = fs::read_dir(paths.state_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b".daemon-proxy-")
        })
        .unwrap();
    let redirect = paths.root().join("outside-policy.json");
    fs::write(&redirect, br#"{"version":1}"#).unwrap();
    fs::remove_file(&entry).unwrap();
    symlink(&redirect, &entry).unwrap();
    assert!(staged.commit().is_err());
    assert_eq!(
        fs::read(paths.state_dir().join("daemon-proxy.json")).unwrap(),
        good
    );
}
