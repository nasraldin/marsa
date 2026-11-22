//! Use Unix filesystem identities and account for macOS's alternate volume paths.

use std::{fs::Metadata, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileId {
    device: u64,
    inode: u64,
}

impl FileId {
    pub(super) fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) const PROTECTED_PATHS: &[&str] = &[
    "/Library/Keychains",
    "/System/Library/Keychains",
    // The inode namespace can name a descendant without showing its parents.
    "/.vol",
];

#[cfg(not(target_os = "macos"))]
pub(super) const PROTECTED_PATHS: &[&str] = &[];

pub(super) fn protected_parent(path: &Path) -> bool {
    // The APFS Data volume can expose protected directories through its
    // unfiltered root as well as through the usual firmlink paths.
    cfg!(target_os = "macos")
        && super::starts_with_insensitive(Path::new("/System/Volumes/Data"), path)
}
