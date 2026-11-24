//! Regression coverage for host path policy using disposable directories.

#![cfg(unix)]

use std::{fs, os::unix::fs::symlink, path::PathBuf};

use marsa_core::paths::MarsaPaths;
use marsa_share::{Allowlist, ShareError, SharePolicy};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    state: PathBuf,
    policy: SharePolicy,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let state = temp.path().join("state/private");
        fs::create_dir(&home).unwrap();
        let policy =
            SharePolicy::new(&home, &MarsaPaths::from_root(state.clone()).unwrap()).unwrap();
        Self {
            _temp: temp,
            home,
            state,
            policy,
        }
    }

    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.home.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

#[test]
fn refuses_protected_paths_their_parents_and_descendants() {
    let f = Fixture::new();
    // These credential paths do not need to exist for their parents to be denied.
    for path in [&f.home, &f.dir("Library"), &f.dir(".config")] {
        assert!(matches!(
            f.policy.validate_share(path),
            Err(ShareError::Protected)
        ));
    }
    for name in [
        ".ssh",
        ".aws",
        ".gnupg",
        ".kube",
        ".docker",
        ".marsa",
        "Library/Keychains",
    ] {
        let private = f.dir(&format!("{name}/nested"));
        let parent = private.parent().unwrap();
        for path in [parent, &private, &parent.join("")] {
            assert!(matches!(
                f.policy.validate_share(path),
                Err(ShareError::Protected)
            ));
        }
    }
    let case_variant = f.dir(".SsH/elsewhere");
    assert!(matches!(
        f.policy.validate_share(&case_variant),
        Err(ShareError::Protected)
    ));
    assert!(f.policy.validate_share(&f.dir(".ssh-samples")).is_ok());

    fs::create_dir_all(&f.state).unwrap();
    for path in [&f.state, f.state.parent().unwrap()] {
        assert!(matches!(
            f.policy.validate_share(path),
            Err(ShareError::Protected)
        ));
    }
}

#[test]
fn denies_aliases_in_both_directions() {
    let f = Fixture::new();
    let store = f.dir("external-store");
    symlink(&store, f.home.join(".aws")).unwrap();
    assert!(matches!(
        f.policy.validate_share(&store),
        Err(ShareError::Protected)
    ));

    let private = f.dir(".ssh");
    let alternative = f.home.join("other-name");
    symlink(&private, &alternative).unwrap();
    assert!(matches!(
        f.policy.validate_share(&alternative),
        Err(ShareError::Protected)
    ));

    let project = f.dir("project");
    symlink(&project, private.join("link")).unwrap();
    assert!(matches!(
        f.policy.validate_share(&private.join("link")),
        Err(ShareError::Protected)
    ));
}

#[test]
fn an_attached_directory_never_authorizes_its_redirected_or_replaced_path() {
    let f = Fixture::new();
    let original = f.dir("original").canonicalize().unwrap();
    let other = f.dir("other").canonicalize().unwrap();
    let mounted = Allowlist::from_paths(f.policy.clone(), [&original, &other]).unwrap();
    let approval = Allowlist::from_paths(f.policy, [&original]).unwrap();
    assert!(
        mounted
            .authorize_if(&original, |root| root == original)
            .is_ok()
    );

    let detached = original.with_file_name("detached");
    fs::rename(&original, detached).unwrap();
    symlink(&other, &original).unwrap();
    assert!(matches!(
        mounted.authorize_if(&other, |root| root == original),
        Err(ShareError::NotAllowed)
    ));
    assert!(approval.verify_roots().is_err());

    fs::remove_file(&original).unwrap();
    fs::create_dir(&original).unwrap();
    assert!(matches!(
        mounted.authorize(&original),
        Err(ShareError::NotAllowed)
    ));
    assert!(approval.verify_roots().is_err());
}

#[test]
fn checks_containment_on_canonical_components_and_refuses_traversal() {
    let f = Fixture::new();
    let configured = f.dir("project");
    let nested = f.dir("project/src");
    let file = nested.join("main.rs");
    fs::write(&file, "").unwrap();
    let other = f.dir("project-old");
    symlink(&other, nested.join("external")).unwrap();
    symlink(&file, nested.join("internal")).unwrap();
    let alias = f.home.join("project-alias");
    symlink(&configured, &alias).unwrap();
    let allowlist = Allowlist::from_paths(f.policy.clone(), [&alias]).unwrap();

    assert_eq!(allowlist.roots(), &[configured.canonicalize().unwrap()]);
    for path in [&file, &nested.join("internal")] {
        assert_eq!(
            allowlist.authorize(path).unwrap(),
            file.canonicalize().unwrap()
        );
    }
    for path in [&other.join(""), &nested.join("external/")] {
        assert!(matches!(
            allowlist.authorize(path),
            Err(ShareError::NotAllowed)
        ));
    }
    assert!(matches!(
        allowlist.authorize(&configured.join("../project/src")),
        Err(ShareError::InvalidPath)
    ));
    if let Ok(variant) = f.home.join("PrOjEcT/src").canonicalize() {
        assert_eq!(
            allowlist.authorize(&variant).unwrap(),
            nested.canonicalize().unwrap()
        );
    }
    assert!(matches!(
        allowlist.authorize(&configured.join("missing")),
        Err(ShareError::Resolve(_))
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn refuses_apfs_data_root_and_alternate_paths_to_protected_directories() {
    use std::path::Path;

    let f = Fixture::new();
    let private = f.dir(".ssh").canonicalize().unwrap();
    let data = Path::new("/System/Volumes/Data");
    let alternate = data.join(private.strip_prefix("/").unwrap());
    assert!(matches!(
        f.policy.validate_share(data),
        Err(ShareError::Protected)
    ));
    if alternate.is_dir() {
        assert!(matches!(
            f.policy.validate_share(&alternate),
            Err(ShareError::Protected)
        ));
    }
}

#[test]
fn denies_a_replaced_approved_root_until_it_is_approved_again() {
    let f = Fixture::new();
    let project = f.dir("project");
    let grants = Allowlist::from_paths(f.policy.clone(), [&project]).unwrap();
    let relocated = f.home.join("relocated");
    fs::rename(&project, &relocated).unwrap();
    fs::create_dir(&project).unwrap();
    for path in [&project, &relocated] {
        assert!(matches!(
            grants.authorize(path),
            Err(ShareError::NotAllowed)
        ));
    }
}

#[test]
fn fails_closed_when_a_protected_symlink_cannot_be_resolved() {
    let f = Fixture::new();
    let project = f.dir("project");
    symlink(f.home.join("missing-store"), f.home.join(".gnupg")).unwrap();
    assert!(matches!(
        f.policy.validate_share(&project),
        Err(ShareError::Protection(_))
    ));
}

#[test]
fn authorizing_regular_files_still_checks_protected_identity() {
    let f = Fixture::new();
    let project = f.dir("project");
    let file = project.join("credential-copy");
    let credential = f.home.join(".git-credentials");
    fs::write(&credential, "").unwrap();
    fs::hard_link(credential, &file).unwrap();

    let grants = Allowlist::from_paths(f.policy.clone(), [&project]).unwrap();
    assert!(matches!(
        grants.authorize(&file),
        Err(ShareError::Protected)
    ));
    fs::remove_file(&file).unwrap();
    fs::write(&file, "").unwrap();
    assert!(grants.authorize(&file).is_ok());
    assert!(matches!(
        f.policy.validate_share(&file),
        Err(ShareError::NotDirectory)
    ));
}

#[test]
fn settings_offer_no_protected_path_override() {
    use marsa_core::settings::Settings;

    assert!(Settings::from_toml("protected_paths = []").is_err());
}
