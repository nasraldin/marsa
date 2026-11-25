//! The saved Git references are bounded, owner-only and limited to HTTPS.

use std::os::unix::fs::PermissionsExt;

use crate::git_sources::{GitRepositoryConfig, GitRepositoryStore};

fn config(url: &str, branch: &str) -> GitRepositoryConfig {
    GitRepositoryConfig {
        name: "Example".into(),
        url: url.into(),
        branch: branch.into(),
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
