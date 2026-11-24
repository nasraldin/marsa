//! Validate explicit host folder grants before a consumer makes them visible to the guest.

#![cfg(unix)]

mod platform;

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use marsa_core::paths::{MarsaPaths, validate_directory};
use thiserror::Error;

use platform::FileId;

const HOME_CREDENTIALS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".docker",
    ".azure",
    ".config/gcloud",
    ".config/gh",
    ".netrc",
    ".git-credentials",
    ".password-store",
    "Library/Keychains",
];

/// Why a share or a bind source was rejected.
#[derive(Debug, Error)]
pub enum ShareError {
    /// The path is relative, root, non-UTF-8, or includes traversal or NUL bytes.
    #[error(
        "share paths must be absolute, non-root UTF-8 paths without parent traversal or NUL bytes"
    )]
    InvalidPath,
    /// The source could not be resolved, or its identity could not be checked.
    #[error("could not resolve the host path")]
    Resolve(#[source] io::Error),
    /// The daemon could not establish what a protected path refers to.
    #[error("could not verify protected host paths")]
    Protection(#[source] io::Error),
    /// The path would include or expose a protected host path.
    #[error("the host path overlaps a protected directory or credential store")]
    Protected,
    /// An allowlist entry must name an existing directory.
    #[error("shared folders must be directories")]
    NotDirectory,
    /// A bind source must be a file or directory.
    #[error("bind sources must be regular files or directories")]
    UnsupportedSource,
    /// The bind source is outside all approved roots.
    #[error("the host path is outside the shared folders")]
    NotAllowed,
    /// The selected directories cannot all be attached at guest boot.
    #[error("shared folders exceed the packaged guest's location, count or path length limits")]
    GuestLayout,
}

/// System-defined protection rules for a user and the daemon's writable state.
#[derive(Debug, Clone)]
pub struct SharePolicy {
    protected: Vec<PathBuf>,
}

impl SharePolicy {
    /// Build protection rules when the configured and account homes are the same.
    pub fn new(home: &Path, paths: &MarsaPaths) -> Result<Self, ShareError> {
        Self::for_account(home, home, paths)
    }

    /// Build protection rules for both the configured and effective account homes.
    pub fn for_account(
        home: &Path,
        account_home: &Path,
        paths: &MarsaPaths,
    ) -> Result<Self, ShareError> {
        let mut protected = vec![paths.root().to_owned()];
        for home in std::iter::once(home).chain((account_home != home).then_some(account_home)) {
            let default = MarsaPaths::from_home(home).map_err(|_| ShareError::InvalidPath)?;
            if paths.root() != default.root() {
                protected.push(default.root().to_owned());
            }
            protected.extend(HOME_CREDENTIALS.iter().map(|path| home.join(path)));
        }
        protected.extend(platform::PROTECTED_PATHS.iter().map(PathBuf::from));
        Ok(Self { protected })
    }

    /// Validate a grant and return the canonical host directory to share.
    pub fn validate_share(&self, path: &Path) -> Result<PathBuf, ShareError> {
        self.directory(path).map(|resolved| resolved.path)
    }

    /// Find the exact directory to request when a regular file or directory is unshared.
    pub fn bind_approval_directory(&self, source: &Path) -> Result<PathBuf, ShareError> {
        let (resolved, file_type) = self.check(source)?;
        if resolved.path != source {
            return Err(ShareError::InvalidPath);
        }
        if file_type.is_dir() {
            return Ok(resolved.path);
        }
        if !file_type.is_file() {
            return Err(ShareError::UnsupportedSource);
        }
        let parent = resolved.path.parent().ok_or(ShareError::InvalidPath)?;
        self.validate_share(parent)
    }

    fn directory(&self, path: &Path) -> Result<Resolved, ShareError> {
        let (resolved, file_type) = self.check(path)?;
        if !file_type.is_dir() {
            return Err(ShareError::NotDirectory);
        }
        Ok(resolved)
    }

    fn check(&self, path: &Path) -> Result<(Resolved, fs::FileType), ShareError> {
        validate_directory(path).map_err(|_| ShareError::InvalidPath)?;
        if path.to_str().is_none() {
            return Err(ShareError::InvalidPath);
        }

        // Check the supplied name as well as the target of any aliases.
        if self
            .protected
            .iter()
            .any(|entry| paths_overlap(path, entry))
            || platform::protected_parent(path)
        {
            return Err(ShareError::Protected);
        }
        let canonical = path.canonicalize().map_err(ShareError::Resolve)?;
        if canonical.to_str().is_none() {
            return Err(ShareError::InvalidPath);
        }
        let metadata = canonical.metadata().map_err(ShareError::Resolve)?;
        let resolved =
            Resolved::from_anchor(&canonical, Path::new("")).map_err(ShareError::Resolve)?;
        if platform::protected_parent(&resolved.path) {
            return Err(ShareError::Protected);
        }

        // An outward symlink beneath a protected directory still entered that
        // directory. Check the supplied ancestry as well as the resolved one.
        let original = (path != canonical)
            .then(|| Resolved::from_anchor(path, Path::new("")))
            .transpose()
            .map_err(ShareError::Resolve)?;
        for entry in &self.protected {
            let protected = Resolved::protected(entry).map_err(ShareError::Protection)?;
            if resolved.overlaps(&protected)
                || original
                    .as_ref()
                    .is_some_and(|original| original.overlaps(&protected))
            {
                return Err(ShareError::Protected);
            }
        }
        Ok((resolved, metadata.file_type()))
    }
}

/// A set of validated host folders, with filesystem identities for containment checks.
#[derive(Debug)]
pub struct Allowlist {
    policy: SharePolicy,
    roots: Vec<PathBuf>,
    identities: Vec<FileId>,
}

impl Allowlist {
    /// Revalidate configured paths and deduplicate directories with the same identity.
    pub fn from_paths<P: AsRef<Path>>(
        policy: SharePolicy,
        paths: impl IntoIterator<Item = P>,
    ) -> Result<Self, ShareError> {
        let mut allowlist = Self {
            policy,
            roots: Vec::new(),
            identities: Vec::new(),
        };
        for path in paths {
            allowlist.add(path.as_ref())?;
        }
        Ok(allowlist)
    }

    /// Approve a directory and return the canonical path stored for its identity.
    pub fn add(&mut self, path: &Path) -> Result<&Path, ShareError> {
        let resolved = self.policy.directory(path)?;
        let identity = resolved.identity();
        let index = self
            .identities
            .iter()
            .position(|previous| *previous == identity);
        let index = index.unwrap_or_else(|| {
            let index = self.roots.len();
            self.roots.push(resolved.path);
            self.identities.push(identity);
            index
        });
        Ok(&self.roots[index])
    }

    /// The canonical approved directories. Revalidate before attaching a filesystem share.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Validate and return the minimum mount set needed in the packaged VM.
    pub fn guest_roots(&self) -> Result<Vec<&PathBuf>, ShareError> {
        use marsa_core::guest_boot::{
            ARM64_CMDLINE_BYTES, PACKAGED_BOOT_ARGS, Settings, append_share_arg,
        };

        let roots: Vec<_> = self
            .roots
            .iter()
            .filter(|path| {
                !self
                    .roots
                    .iter()
                    .any(|other| *path != other && path.starts_with(other))
            })
            .collect();
        let mut args = PACKAGED_BOOT_ARGS.to_owned();
        for (index, path) in roots.iter().enumerate() {
            append_share_arg(&mut args, &format!("marsa-share-{index}"), path)
                .map_err(|_| ShareError::GuestLayout)?;
            if args.len() >= ARM64_CMDLINE_BYTES {
                return Err(ShareError::GuestLayout);
            }
        }
        Settings::parse(&args).map_err(|_| ShareError::GuestLayout)?;
        Ok(roots)
    }

    /// Reject approval if any recorded directory has moved, changed identity or become protected.
    pub fn verify_roots(&self) -> Result<(), ShareError> {
        for (root, identity) in self.roots.iter().zip(&self.identities) {
            let current = self.policy.directory(root)?;
            if current.path != *root || current.identity() != *identity {
                return Err(ShareError::NotAllowed);
            }
        }
        Ok(())
    }

    /// Check an existing bind source and return its canonical path for the consumer to use.
    pub fn authorize(&self, source: &Path) -> Result<PathBuf, ShareError> {
        self.authorize_if(source, |_| true)
    }

    /// Check a bind against previously resolved root identities still approved by the caller.
    pub fn authorize_if(
        &self,
        source: &Path,
        approved: impl Fn(&Path) -> bool,
    ) -> Result<PathBuf, ShareError> {
        let (resolved, file_type) = self.policy.check(source)?;
        if !file_type.is_dir() && !file_type.is_file() {
            return Err(ShareError::UnsupportedSource);
        }
        for (root, identity) in self.roots.iter().zip(&self.identities) {
            if approved(root)
                && resolved.ancestry.iter().any(|(found, _)| found == identity)
                && root.canonicalize().is_ok_and(|current| current == *root)
                && root
                    .metadata()
                    .is_ok_and(|metadata| FileId::of(&metadata) == *identity)
            {
                return Ok(resolved.path);
            }
        }
        Err(ShareError::NotAllowed)
    }
}

struct Resolved {
    path: PathBuf,
    // Each existing ancestor's identity and the path from it to the requested
    // object. On APFS, filesystem identity also accounts for case, Unicode
    // normalization, and firmlinks.
    ancestry: Vec<(FileId, PathBuf)>,
}

impl Resolved {
    fn from_anchor(anchor: &Path, missing: &Path) -> io::Result<Self> {
        let path = if missing.as_os_str().is_empty() {
            anchor.to_owned()
        } else {
            anchor.join(missing)
        };
        let ancestry = Self::ancestors(anchor, &path)?;
        Ok(Self { path, ancestry })
    }

    fn ancestors(anchor: &Path, path: &Path) -> io::Result<Vec<(FileId, PathBuf)>> {
        anchor
            .ancestors()
            .map(|ancestor| {
                let identity = FileId::of(&ancestor.metadata()?);
                // Both paths are built from anchor, so the ancestor is a prefix.
                let suffix = path.strip_prefix(ancestor).map_err(io::Error::other)?;
                Ok((identity, suffix.to_owned()))
            })
            .collect()
    }

    fn protected(path: &Path) -> io::Result<Self> {
        // Resolve any existing part, even when the credential store hasn't been
        // created yet. A symlink with an unreachable target fails the check.
        for ancestor in path.ancestors() {
            match ancestor.symlink_metadata() {
                Ok(_) => {
                    let suffix = path.strip_prefix(ancestor).map_err(io::Error::other)?;
                    let canonical = ancestor.canonicalize()?;
                    let mut resolved = Self::from_anchor(&canonical, suffix)?;
                    if canonical != ancestor {
                        resolved.ancestry.extend(Self::ancestors(ancestor, path)?);
                    }
                    return Ok(resolved);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        Err(io::ErrorKind::NotFound.into())
    }

    fn identity(&self) -> FileId {
        // A resolved absolute path has at least the anchor as an ancestor.
        self.ancestry[0].0
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.ancestry.iter().any(|(id, tail)| {
            other
                .ancestry
                .iter()
                .any(|(other_id, other_tail)| id == other_id && paths_overlap(tail, other_tail))
        })
    }
}

fn paths_overlap(first: &Path, second: &Path) -> bool {
    starts_with_insensitive(first, second) || starts_with_insensitive(second, first)
}

fn starts_with_insensitive(path: &Path, prefix: &Path) -> bool {
    let mut components = path.components();
    prefix.components().all(|expected| {
        components.next().is_some_and(|actual| {
            actual
                .as_os_str()
                .as_encoded_bytes()
                .eq_ignore_ascii_case(expected.as_os_str().as_encoded_bytes())
        })
    })
}
