//! Verify the exact advertised and checked-out branch without trusting Git's pattern matching.

use std::{path::Path, process::Stdio, time::Duration};

use nix::libc;
use tokio::{fs, io::AsyncReadExt as _, process::Command, time::timeout};

use super::GitSourceError;

const ADVERTISEMENT_LIMIT: usize = 4 * 1024 * 1024;
const HEAD_LIMIT: usize = 1024;

/// Check the requested branch among the output of a configured Git ls-remote command.
pub(super) async fn advertised(command: &mut Command, branch: &str) -> Result<(), GitSourceError> {
    timeout(Duration::from_secs(20), async {
        let mut child = command
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| GitSourceError::Git)?;
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or(GitSourceError::Git)?
            .take(ADVERTISEMENT_LIMIT as u64 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| GitSourceError::Git)?;
        if output.len() > ADVERTISEMENT_LIMIT {
            return Err(GitSourceError::Git);
        }

        let status = child.wait().await.map_err(|_| GitSourceError::Git)?;
        match status.code() {
            Some(0) => exact_advertisement(&output, branch),
            Some(2) => Err(GitSourceError::BranchMissing),
            _ => Err(GitSourceError::Git),
        }
    })
    .await
    .map_err(|_| GitSourceError::Timeout)?
}

fn exact_advertisement(output: &[u8], branch: &str) -> Result<(), GitSourceError> {
    let expected = format!("refs/heads/{branch}");
    for record in output.split_inclusive(|byte| *byte == b'\n') {
        let Some(line) = record.strip_suffix(b"\n") else {
            return Err(GitSourceError::Git);
        };
        let Some(separator) = line.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if &line[separator + 1..] == expected.as_bytes() {
            let hash = &line[..separator];
            return if matches!(hash.len(), 40 | 64)
                && hash.iter().all(u8::is_ascii_hexdigit)
                && hash.iter().any(|byte| *byte != b'0')
            {
                Ok(())
            } else {
                Err(GitSourceError::Git)
            };
        }
    }
    Err(GitSourceError::BranchMissing)
}

/// Check that clone created an ordinary symbolic HEAD for the requested branch.
pub(super) async fn checked_out(workspace: &Path, branch: &str) -> Result<(), GitSourceError> {
    let git_dir = workspace.join(".git");
    if !fs::symlink_metadata(&git_dir)
        .await
        .map_err(|_| GitSourceError::Git)?
        .is_dir()
    {
        return Err(GitSourceError::Git);
    }
    let head = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(git_dir.join("HEAD"))
        .await
        .map_err(|_| GitSourceError::Git)?;
    if !head
        .metadata()
        .await
        .map_err(|_| GitSourceError::Git)?
        .is_file()
    {
        return Err(GitSourceError::Git);
    }
    let mut value = Vec::new();
    head.take(HEAD_LIMIT as u64 + 1)
        .read_to_end(&mut value)
        .await
        .map_err(|_| GitSourceError::Git)?;
    if value.len() > HEAD_LIMIT {
        return Err(GitSourceError::Git);
    }
    if value == format!("ref: refs/heads/{branch}\n").as_bytes() {
        Ok(())
    } else {
        Err(GitSourceError::BranchMissing)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    const SHA1: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    const SHA256: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    #[test]
    fn distinguishes_exact_branches_from_suffixes_and_other_ref_fields() {
        let suffixes = format!(
            "{SHA1}\trefs/heads/nested/refs/heads/main\n{SHA256}\trefs/tags/refs/heads/main\n"
        );
        assert!(matches!(
            exact_advertisement(suffixes.as_bytes(), "main"),
            Err(GitSourceError::BranchMissing)
        ));
        for hash in [SHA1, SHA256] {
            let exact = format!("{suffixes}{hash}\trefs/heads/feature/main\n");
            assert!(exact_advertisement(exact.as_bytes(), "feature/main").is_ok());
        }
        for hash in ["", "not-a-hash", &"0".repeat(40)] {
            let malformed = format!("{hash}\trefs/heads/main\n");
            assert!(matches!(
                exact_advertisement(malformed.as_bytes(), "main"),
                Err(GitSourceError::Git)
            ));
        }
    }

    #[tokio::test]
    async fn checks_advertisement_content_and_transport_status() {
        for (reference, status) in [
            ("refs/heads/main", "0"),
            ("refs/tags/refs/heads/main", "0"),
            ("refs/heads/main", "2"),
            ("refs/heads/main", "1"),
        ] {
            let mut command = Command::new("/bin/sh");
            command
                .env_clear()
                .args([
                    "-c",
                    "printf '%s\\t%s\\n' \"$1\" \"$2\"; exit \"$3\"",
                    "fixture",
                ])
                .args([SHA1, reference, status]);
            let result = advertised(&mut command, "main").await;
            match (reference, status) {
                ("refs/heads/main", "0") => assert!(result.is_ok()),
                (_, "0" | "2") => assert!(matches!(result, Err(GitSourceError::BranchMissing))),
                _ => assert!(matches!(result, Err(GitSourceError::Git))),
            }
        }
    }

    #[tokio::test]
    async fn refuses_detached_wrong_or_symlinked_checkout_head() {
        let directory = tempfile::tempdir().unwrap();
        let git_dir = directory.path().join(".git");
        fs::create_dir(&git_dir).await.unwrap();
        let head = git_dir.join("HEAD");
        for value in [
            SHA1,
            "ref: refs/tags/main\n",
            "ref: refs/heads/other/main\n",
        ] {
            fs::write(&head, value).await.unwrap();
            assert!(matches!(
                checked_out(directory.path(), "main").await,
                Err(GitSourceError::BranchMissing)
            ));
        }
        fs::write(&head, "ref: refs/heads/main\n").await.unwrap();
        assert!(checked_out(directory.path(), "main").await.is_ok());

        let alternate = directory.path().join("alternate");
        fs::rename(&head, &alternate).await.unwrap();
        symlink(alternate, &head).unwrap();
        assert!(matches!(
            checked_out(directory.path(), "main").await,
            Err(GitSourceError::Git)
        ));
        let linked = tempfile::tempdir().unwrap();
        symlink(git_dir, linked.path().join(".git")).unwrap();
        assert!(matches!(
            checked_out(linked.path(), "main").await,
            Err(GitSourceError::Git)
        ));
    }
}
