//! Load the approved boot policy independently of the editable settings document.

use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, ProxySettings, Settings},
};
use marsa_proto::guest::v1::{ConfigureNetwork, Proxy};
use nix::{
    errno::Errno,
    fcntl::{OFlag, openat, renameat},
    sys::stat::Mode,
};
use serde::{Deserialize, Serialize};

use crate::factory::PROXY_POLICY_FILENAME as FILENAME;
const MAX_BYTES: u64 = 4096;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct BootProxy(ProxySettings);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    http: Option<String>,
    https: Option<String>,
}

impl BootProxy {
    pub(super) fn read(paths: &MarsaPaths, uid: u32) -> io::Result<Self> {
        let state = open_state(&paths.state_dir(), uid)?;
        let Some(file) = open_record(&state, uid)? else {
            return Ok(Self::default());
        };

        // Discard parser diagnostics so a malformed record cannot put URLs in logs.
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| io::ErrorKind::InvalidData)?;
        if record.version != 1 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Self::from_draft(&ProxySettings {
            http: record.http,
            https: record.https,
            inject_into_containers: false,
        })
    }

    pub(super) fn from_draft(proxy: &ProxySettings) -> io::Result<Self> {
        if proxy.inject_into_containers {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let policy = Self(proxy.clone());
        policy
            .effective(Settings::default())
            .validate()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        Ok(policy)
    }

    pub(super) fn stage(&self, paths: &MarsaPaths, uid: u32) -> io::Result<StagedProxy> {
        let directory = paths.state_dir();
        let state = open_state(&directory, uid)?;
        open_record(&state, uid)?;
        let mut record = tempfile::Builder::new()
            .prefix(".daemon-proxy-")
            .tempfile_in(&directory)?;
        record
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(
            &mut record,
            &Record {
                version: 1,
                http: self.0.http.clone(),
                https: self.0.https.clone(),
            },
        )
        .map_err(io::Error::other)?;
        record.flush()?;
        record.as_file().sync_all()?;
        Ok(StagedProxy {
            directory,
            state,
            record,
            uid,
        })
    }

    pub(super) fn effective(&self, mut desired: Settings) -> Settings {
        if self.has_proxy() {
            // An approved proxy remains attached to the selected user-space path.
            // A draft attachment change must not move or silently drop that proxy.
            desired.network.mode = NetworkMode::Userspace;
        }
        desired.network.proxy.clone_from(&self.0);
        desired
    }

    pub(super) fn effective_on_userspace(&self, mut settings: Settings) -> Settings {
        settings.network.mode = NetworkMode::Userspace;
        self.effective(settings)
    }

    pub(super) fn guest_configuration(&self) -> Option<ConfigureNetwork> {
        self.has_proxy().then(|| ConfigureNetwork {
            proxy: Some(Proxy {
                http_url: self.0.http.clone(),
                https_url: self.0.https.clone(),
                no_proxy: Vec::new(),
            }),
            ..Default::default()
        })
    }

    fn has_proxy(&self) -> bool {
        self.0.http.is_some() || self.0.https.is_some()
    }
}

pub(super) struct StagedProxy {
    directory: PathBuf,
    state: File,
    record: tempfile::NamedTempFile,
    uid: u32,
}

impl StagedProxy {
    /// The boolean reports whether the committed directory entry was synchronized.
    pub(super) fn commit(self) -> io::Result<bool> {
        let actual = open_state(&self.directory, self.uid)?.metadata()?;
        let expected = self.state.metadata()?;
        if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        open_record(&self.state, self.uid)?;
        let staged_name = self
            .record
            .path()
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?;
        let staged_entry = open_owned_file(&self.state, staged_name, self.uid)?
            .ok_or(io::ErrorKind::NotFound)?
            .metadata()?;
        let staged_file = self.record.as_file().metadata()?;
        if staged_entry.dev() != staged_file.dev() || staged_entry.ino() != staged_file.ino() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        renameat(&self.state, staged_name, &self.state, FILENAME)?;
        // Once renamed, the visible commit and the live policy must agree even
        // if the filesystem cannot confirm directory-entry durability.
        Ok(self.state.sync_all().is_ok())
    }
}

fn open_state(path: &Path, uid: u32) -> io::Result<File> {
    let state = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_DIRECTORY).bits())
        .open(path)?;
    let directory = state.metadata()?;
    if directory.uid() != uid || directory.mode() & 0o7777 != 0o700 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(state)
}

fn open_record(state: &File, uid: u32) -> io::Result<Option<File>> {
    open_owned_file(state, OsStr::new(FILENAME), uid)
}

fn open_owned_file(state: &File, name: &OsStr, uid: u32) -> io::Result<Option<File>> {
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = match openat(state, name, flags, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != uid || info.mode() & 0o7777 != 0o600 || info.nlink() != 1 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(Some(file))
}

pub(super) struct RuntimeProxy(Mutex<RuntimeState>);

struct RuntimeState {
    approved: BootProxy,
    candidate: Option<Settings>,
}

impl RuntimeProxy {
    pub(super) fn new(approved: BootProxy) -> Self {
        Self(Mutex::new(RuntimeState {
            approved,
            candidate: None,
        }))
    }

    pub(super) fn approved(&self) -> BootProxy {
        self.state().approved.clone()
    }

    pub(super) fn replace_approved(&self, policy: BootProxy) {
        self.state().approved = policy;
    }

    pub(super) fn has_approved_proxy(&self) -> bool {
        self.state().approved.has_proxy()
    }

    pub(super) fn launch_settings(&self, desired: Settings) -> io::Result<(Settings, bool)> {
        let state = self.state();
        match &state.candidate {
            Some(candidate) => Ok((candidate.clone(), true)),
            None if state.approved.has_proxy()
                && desired.network.mode != NetworkMode::Userspace =>
            {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "approved proxy requires user-space networking",
                ))
            }
            None => Ok((state.approved.effective(desired), false)),
        }
    }

    pub(super) fn override_for_permit(&self, settings: Settings) -> LaunchOverride<'_> {
        self.state().candidate = Some(settings);
        LaunchOverride(self)
    }

    fn state(&self) -> MutexGuard<'_, RuntimeState> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}

pub(super) struct LaunchOverride<'a>(&'a RuntimeProxy);

impl Drop for LaunchOverride<'_> {
    fn drop(&mut self) {
        self.0.state().candidate = None;
    }
}
