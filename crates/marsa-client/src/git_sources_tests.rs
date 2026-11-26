//! The saved Git references are bounded, owner-only and transport-scoped.

use std::os::unix::fs::PermissionsExt;

use crate::git_sources::{GitRepositoryConfig, GitRepositoryStore, GitSourceError};

fn config(url: &str, branch: &str) -> GitRepositoryConfig {
    GitRepositoryConfig {
        name: "Example".into(),
        url: url.into(),
        branch: branch.into(),
        ssh: None,
    }
}

#[test]
fn references_cannot_enable_local_protocols_or_smuggle_credentials() {
    for url in [
        "/tmp/source",
        "file:///tmp/source",
        "ext::sh -c something",
        "https://person:secret@git.example/repo",
        "https://git.example/repo?token=secret",
        "https://git.example/repo#branch",
    ] {
        assert!(config(url, "main").validate().is_err(), "{url}");
    }
    for branch in [
        "--upload-pack=other",
        "bad..ref",
        "branch.lock",
        "branch one",
    ] {
        assert!(
            config("https://git.example/repo.git", branch)
                .validate()
                .is_err(),
            "{branch}"
        );
    }
    assert!(
        config("https://git.example:443/org/repo.git", "release/v2")
            .validate()
            .is_ok()
    );
}

#[tokio::test]
async fn checked_repository_edits_survive_reopen_and_do_not_change_id() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("marsa");
    let store = GitRepositoryStore::at(directory.clone());
    let id = store
        .save(None, config("https://git.example/repo", "main"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(directory.join("git-repositories.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let reopened = GitRepositoryStore::at(directory);
    assert_eq!(reopened.list().await.unwrap()[0].id, id);
    let edited = reopened
        .save(Some(&id), config("https://git.example/repo", "feature/new"))
        .await
        .unwrap();
    assert_eq!(edited, id);
    assert_eq!(
        reopened.list().await.unwrap()[0].config.branch,
        "feature/new"
    );
    reopened.remove(&id).await.unwrap();
    assert!(reopened.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn stale_credential_review_cannot_edit_the_replacement() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("marsa");
    let store = GitRepositoryStore::at(directory.clone());
    store
        .save(None, config("https://git.example/repo", "main"))
        .await
        .unwrap();
    let path = directory.join("git-repositories.json");
    let mut entries: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    entries[0]["credential_revision"] = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".into();
    std::fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();
    let reviewed = store.list().await.unwrap().remove(0);
    entries[0]["credential_revision"] = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb".into();
    let replacement = serde_json::to_vec(&entries).unwrap();
    std::fs::write(&path, &replacement).unwrap();
    assert!(matches!(
        store.remove_https_credential(&reviewed).await,
        Err(GitSourceError::CredentialChanged)
    ));
    assert!(matches!(
        store
            .save_https_credential(&reviewed, "fixture".into(), "dummy".into())
            .await,
        Err(GitSourceError::CredentialChanged)
    ));
    assert_eq!(std::fs::read(path).unwrap(), replacement);
}

#[tokio::test]
#[ignore = "requires network access to the public GitHub repository"]
async fn tests_and_checks_out_a_public_https_branch() {
    let root = tempfile::tempdir().unwrap();
    let source = config("https://github.com/octocat/Hello-World.git", "master");
    source.test().await.unwrap();
    let missing = config(
        "https://github.com/octocat/Hello-World.git",
        "marsa-nonexistent-fixture-branch",
    );
    assert!(matches!(
        missing.test().await,
        Err(crate::git_sources::GitSourceError::BranchMissing)
    ));
    let path = source.checkout(root.path()).await.unwrap();
    assert_eq!(
        path.parent(),
        Some(root.path().canonicalize().unwrap().as_path())
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(path.join("README").is_file());
    let second = source.checkout(root.path()).await.unwrap();
    assert_ne!(path, second);
    assert!(path.join("README").is_file());
}

#[test]
fn ssh_sources_require_explicit_safe_addresses_and_pins() {
    use crate::git_sources::GitSshAuthentication;

    let mut source = config("ssh://git@example.invalid:2222/team/repo.git", "main");
    assert!(
        source.validate().is_err(),
        "a saved SSH URL needs explicit credentials"
    );
    source.ssh = Some(GitSshAuthentication {
        host_key:
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB"
                .into(),
        identity_file: "/path with spaces/identity".into(),
        identity_fingerprint: "SHA256:AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".into(),
    });
    assert!(source.validate().is_ok());
    source.url = "ssh://git@[::1]:2222/team/repo.git".into();
    assert!(source.validate().is_ok());
    for url in [
        "ssh://example.invalid/repo",
        "ssh://user:password@example.invalid/repo",
        "ssh://-option@example.invalid/repo",
        "ssh://user@example.invalid:0/repo",
        "ssh://user@example.invalid:0x16/repo",
        "ssh://user@example.invalid/~repo",
        "ssh://user@example.invalid/team/../repo",
        "ssh://user@example.invalid/team//repo",
        "ssh://user@example.invalid/team/repo?x",
        "ssh://user@example.invalid/team/repo%0a",
        "ssh://user@example.invalid/team/repo;",
        "git@example.invalid:team/repo",
        "https://example.invalid/team/repo",
    ] {
        source.url = url.into();
        assert!(source.validate().is_err(), "{url}");
    }
}

#[tokio::test]
#[ignore = "requires the disposable loopback SSH Git fixture"]
async fn authenticated_ssh_uses_the_saved_transport_and_rejects_changes() {
    use std::{path::PathBuf, process::Command};

    use crate::git_sources::{GitSshAuthentication, review_git_ssh_identity};

    #[derive(serde::Deserialize)]
    struct Fixture {
        url: String,
        host_key: String,
        identity_file: PathBuf,
        wrong_identity_file: PathBuf,
        requests_log: PathBuf,
    }

    let manifest = PathBuf::from(std::env::var_os("MARSA_GIT_SSH_FIXTURE").unwrap());
    let fixture: Fixture = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    let requests = || {
        std::fs::read_to_string(&fixture.requests_log)
            .unwrap()
            .lines()
            .count()
    };
    let directory = tempfile::tempdir().unwrap();
    let ssh_key = directory.path().join("selected private key");
    std::fs::copy(&fixture.identity_file, &ssh_key).unwrap();
    std::fs::set_permissions(&ssh_key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let authentication = GitSshAuthentication {
        host_key: fixture.host_key,
        identity_fingerprint: review_git_ssh_identity(&ssh_key).await.unwrap(),
        identity_file: ssh_key.clone(),
    };
    let mut source = config(&fixture.url, "main");
    source.ssh = Some(authentication.clone());
    let old_requests = requests();
    source.test().await.unwrap();
    assert!(requests() > old_requests);
    let checkout = source.checkout(directory.path()).await.unwrap();
    assert_eq!(
        std::fs::read(checkout.join("compose.yaml")).unwrap(),
        b"name: ssh-qualification\nservices: {}\n"
    );
    let origin = Command::new("/usr/bin/git")
        .env_clear()
        .current_dir(checkout)
        .args(["config", "--local", "--get", "remote.origin.url"])
        .output()
        .unwrap();
    assert!(origin.status.success());
    assert_eq!(origin.stdout, format!("{}\n", fixture.url).as_bytes());

    let checked_requests = requests();
    let mut wrong_host = authentication.clone();
    wrong_host.host_key = std::fs::read_to_string(manifest.with_file_name("wrong.pub"))
        .unwrap()
        .trim()
        .into();
    source.ssh = Some(wrong_host);
    assert!(matches!(
        source.test().await,
        Err(GitSourceError::SshConnection)
    ));
    assert_eq!(requests(), checked_requests);

    std::fs::copy(&fixture.wrong_identity_file, &ssh_key).unwrap();
    source.ssh = Some(authentication.clone());
    assert!(matches!(
        source.test().await,
        Err(GitSourceError::SshIdentityChanged)
    ));
    assert_eq!(requests(), checked_requests);

    source.ssh = Some(GitSshAuthentication {
        identity_fingerprint: review_git_ssh_identity(&ssh_key).await.unwrap(),
        ..authentication
    });
    assert!(matches!(
        source.test().await,
        Err(GitSourceError::SshConnection)
    ));
    assert_eq!(requests(), checked_requests);

    std::fs::set_permissions(&ssh_key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        review_git_ssh_identity(&ssh_key).await,
        Err(GitSourceError::SshIdentity)
    ));
    std::fs::remove_file(&ssh_key).unwrap();
    std::os::unix::fs::symlink(&fixture.identity_file, &ssh_key).unwrap();
    assert!(matches!(
        review_git_ssh_identity(&ssh_key).await,
        Err(GitSourceError::SshIdentity)
    ));
    let encrypted = directory.path().join("encrypted fixture");
    assert!(
        Command::new("/usr/bin/ssh-keygen")
            .env_clear()
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "disposable fixture passphrase",
                "-f"
            ])
            .arg(&encrypted)
            .status()
            .unwrap()
            .success()
    );
    assert!(matches!(
        review_git_ssh_identity(&encrypted).await,
        Err(GitSourceError::SshIdentity)
    ));
}
