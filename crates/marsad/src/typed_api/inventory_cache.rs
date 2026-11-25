//! Durable Docker overview snapshots for fast owner reads (D-034).

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_proto::host::v1::DockerOverviewSection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(super) const CACHE_SECTION_LIMIT: usize = 2 * 1024 * 1024;

/// Sections the daemon may retain between Docker round-trips.
const CACHEABLE: &[DockerOverviewSection] = &[
    DockerOverviewSection::Containers,
    DockerOverviewSection::Images,
    DockerOverviewSection::Volumes,
    DockerOverviewSection::Networks,
    DockerOverviewSection::Disk,
    DockerOverviewSection::Version,
];

/// One cached overview section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CachedSection {
    pub(crate) json: Vec<u8>,
    pub(crate) observed_at_unix_millis: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    sections: BTreeMap<String, CachedSection>,
}

/// File-backed inventory cache owned by `marsad`.
pub(crate) struct InventoryCache {
    path: PathBuf,
    state: Mutex<StoreFile>,
}

#[derive(Debug, Error)]
pub(crate) enum InventoryCacheError {
    #[error("inventory cache path is not absolute")]
    RelativePath,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl InventoryCache {
    /// Open or create the cache file under the daemon state directory.
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, InventoryCacheError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(InventoryCacheError::RelativePath);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let state = if path.exists() {
            let bytes = fs::read(&path)?;
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            StoreFile::default()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    /// Sections refreshed by the background loop (excludes live container stats).
    pub(crate) fn cacheable_sections() -> &'static [DockerOverviewSection] {
        CACHEABLE
    }

    pub(crate) fn is_cacheable(section: DockerOverviewSection) -> bool {
        CACHEABLE.contains(&section)
    }

    pub(crate) fn get(
        &self,
        section: DockerOverviewSection,
    ) -> Result<Option<CachedSection>, InventoryCacheError> {
        let key = section_key(section)?;
        let state = self.state.lock().expect("inventory cache lock");
        Ok(state.sections.get(key).cloned())
    }

    pub(crate) fn put(
        &self,
        section: DockerOverviewSection,
        json: &[u8],
        observed_at_unix_millis: i64,
    ) -> Result<(), InventoryCacheError> {
        let key = section_key(section)?;
        if json.len() > CACHE_SECTION_LIMIT {
            return Err(InventoryCacheError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "overview section exceeds the 2 MiB cache limit",
            )));
        }
        let mut state = self.state.lock().expect("inventory cache lock");
        state.sections.insert(
            key.to_owned(),
            CachedSection {
                json: json.to_vec(),
                observed_at_unix_millis,
            },
        );
        let encoded = serde_json::to_vec_pretty(&*state)?;
        drop(state);
        atomic_write(&self.path, &encoded)?;
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn section_key(section: DockerOverviewSection) -> Result<&'static str, InventoryCacheError> {
    match section {
        DockerOverviewSection::Containers => Ok("containers"),
        DockerOverviewSection::Images => Ok("images"),
        DockerOverviewSection::Volumes => Ok("volumes"),
        DockerOverviewSection::Networks => Ok("networks"),
        DockerOverviewSection::Disk => Ok("disk"),
        DockerOverviewSection::Version => Ok("version"),
        _ => Err(InventoryCacheError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "section is not cacheable",
        ))),
    }
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trips_section_bytes_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("docker-inventory.json");
        let cache = InventoryCache::open(&path).unwrap();
        let payload = br#"[{"Id":"abc"}]"#;
        cache
            .put(DockerOverviewSection::Containers, payload, 42)
            .unwrap();
        drop(cache);

        let reopened = InventoryCache::open(&path).unwrap();
        let cached = reopened
            .get(DockerOverviewSection::Containers)
            .unwrap()
            .unwrap();
        assert_eq!(cached.json, payload);
        assert_eq!(cached.observed_at_unix_millis, 42);
        assert!(
            reopened
                .get(DockerOverviewSection::Images)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_container_stats() {
        let dir = tempdir().unwrap();
        let cache = InventoryCache::open(dir.path().join("cache.json")).unwrap();
        assert!(!InventoryCache::is_cacheable(
            DockerOverviewSection::ContainerStats
        ));
        assert!(
            cache
                .put(DockerOverviewSection::ContainerStats, b"{}", 1)
                .is_err()
        );
    }
}
