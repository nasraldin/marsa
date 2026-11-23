//! Discover native scanner executables in installation paths controlled by the owner or system.

use std::{
    fs::Metadata,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use tokio::{fs, io::AsyncReadExt as _};

pub(crate) async fn scanner_executable(binary: &str) -> Option<PathBuf> {
    let mut candidates = vec![
        Path::new("/opt/homebrew/bin").join(binary),
        Path::new("/usr/local/bin").join(binary),
        Path::new("/usr/bin").join(binary),
    ];
    if let Some(home) = std::env::home_dir() {
        candidates.push(home.join(".local/bin").join(binary));
    }
    for path in candidates {
        if let Some(checked) = trusted_executable(&path).await {
            return Some(checked);
        }
    }
    None
}

async fn trusted_executable(path: &Path) -> Option<PathBuf> {
    let owner = nix::unistd::geteuid().as_raw();
    let entry = fs::symlink_metadata(path).await.ok()?;
    if !trusted_owner(&entry, owner) {
        return None;
    }
    // Homebrew uses a symlink for the executable. Both its search directory
    // and target directory must be controlled by this owner or the system.
    if !trusted_directories(path.parent()?, owner).await {
        return None;
    }
    let canonical = fs::canonicalize(path).await.ok()?;
    if !trusted_directories(canonical.parent()?, owner).await {
        return None;
    }
    let metadata = fs::symlink_metadata(&canonical).await.ok()?;
    if !trusted_owner(&metadata, owner)
        || !metadata.is_file()
        || metadata.mode() & 0o6022 != 0
        || metadata.mode() & 0o111 == 0
    {
        return None;
    }
    let mut magic = [0; 4];
    fs::File::open(&canonical)
        .await
        .ok()?
        .read_exact(&mut magic)
        .await
        .ok()?;
    let native = if cfg!(target_os = "macos") {
        matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe] | [0xca, 0xfe, 0xba, 0xbe] | [0xca, 0xfe, 0xba, 0xbf]
        )
    } else {
        magic == *b"\x7fELF"
    };
    native.then_some(canonical)
}

async fn trusted_directories(directory: &Path, owner: u32) -> bool {
    for directory in directory.ancestors() {
        let Ok(metadata) = fs::symlink_metadata(directory).await else {
            return false;
        };
        if !metadata.is_dir() || !trusted_owner(&metadata, owner) || metadata.mode() & 0o022 != 0 {
            return false;
        }
    }
    true
}

fn trusted_owner(metadata: &Metadata, owner: u32) -> bool {
    metadata.uid() == 0 || metadata.uid() == owner
}
