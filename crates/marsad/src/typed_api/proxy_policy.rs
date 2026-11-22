//! Load the approved boot policy independently of the editable settings document.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

use marsa_core::{
    paths::MarsaPaths,
    settings::{NetworkMode, ProxySettings, Settings},
};
use marsa_proto::guest::v1::{ConfigureNetwork, Proxy};
use nix::{
    errno::Errno,
    fcntl::{OFlag, openat},
    sys::stat::Mode,
};
use serde::Deserialize;

const FILENAME: &str = "daemon-proxy.json";
const MAX_BYTES: u64 = 4096;

#[derive(Debug, Default)]
pub(super) struct BootProxy(ProxySettings);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    http: Option<String>,
    https: Option<String>,
}

impl BootProxy {
    pub(super) fn read(paths: &MarsaPaths, uid: u32) -> io::Result<Self> {
        let state = OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_DIRECTORY).bits())
            .open(paths.state_dir())?;
        let directory = state.metadata()?;
        if directory.uid() != uid || directory.mode() & 0o7777 != 0o700 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
        let file = match openat(&state, FILENAME, flags, Mode::empty()) {
            Ok(fd) => File::from(fd),
            Err(Errno::ENOENT) => return Ok(Self::default()),
            Err(error) => return Err(error.into()),
        };
        let info = file.metadata()?;
        if !info.is_file()
            || info.uid() != uid
            || info.mode() & 0o7777 != 0o600
            || info.nlink() != 1
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }

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
        let policy = Self(ProxySettings {
            http: record.http,
            https: record.https,
            inject_into_containers: false,
        });
        policy
            .effective(Settings::default())
            .validate()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        Ok(policy)
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
