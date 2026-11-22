//! Revalidate persisted grants before exposing any host directory to the VM.

use std::{io, path::PathBuf};

use marsa_share::{Allowlist, ShareError, SharePolicy};
use marsa_vm::ShareSpec;

pub(super) fn configured(policy: &SharePolicy, granted: &[PathBuf]) -> io::Result<Vec<ShareSpec>> {
    let input_error = |error| {
        let kind = if matches!(error, ShareError::Protected) {
            io::ErrorKind::PermissionDenied
        } else {
            io::ErrorKind::InvalidInput
        };
        io::Error::new(kind, error)
    };
    let allowlist = Allowlist::from_paths(policy.clone(), granted).map_err(input_error)?;
    if allowlist.roots().iter().any(|path| !granted.contains(path)) {
        return Err(input_error(ShareError::InvalidPath));
    }
    Ok(allowlist
        .guest_roots()
        .map_err(input_error)?
        .into_iter()
        .enumerate()
        .map(|(index, path)| ShareSpec {
            tag: format!("marsa-share-{index}"),
            host_path: path.clone(),
            read_only: false,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use marsa_core::paths::MarsaPaths;

    use super::*;

    #[test]
    fn attaches_canonical_roots_and_revalidates_configured_paths() {
        let home = tempfile::tempdir().unwrap();
        let paths = MarsaPaths::from_home(home.path()).unwrap();
        let policy = SharePolicy::new(home.path(), &paths).unwrap();
        let project = home.path().canonicalize().unwrap().join("project");
        let nested = project.join("nested");
        let credentials = home.path().join(".ssh");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&credentials).unwrap();

        let specs = configured(&policy, &[nested, project.clone()]).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].host_path, project.canonicalize().unwrap());
        assert!(!specs[0].read_only);
        fs::rename(&project, home.path().join("old-project")).unwrap();
        symlink(credentials, &project).unwrap();
        assert!(configured(&policy, &[project]).is_err());
    }
}
