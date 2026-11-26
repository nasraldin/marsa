//! Apply host resolver, proxy and trust configuration inside the guest only.

pub(super) mod native;
mod resolver;
mod trust;

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};

use marsa_proto::guest::v1::{ConfigureNetwork, InstallCertificates};

const HOSTS: &str = "/etc/hosts";
const RESOLVER: &str = "/etc/resolv.conf";
const DAEMON: &str = "/etc/docker/daemon.json";
const CERTIFICATES: &str = "/etc/ssl/certs/ca-certificates.crt";
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_CA_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Config(Arc<Defaults>);

struct Defaults {
    hosts: String,
    resolver: String,
    certificates: Vec<u8>,
}

impl Config {
    pub(super) fn load() -> io::Result<Self> {
        Ok(Self(Arc::new(Defaults {
            hosts: String::from_utf8(read_or_empty(HOSTS)?).map_err(io::Error::other)?,
            resolver: String::from_utf8(read_or_empty(RESOLVER)?).map_err(io::Error::other)?,
            certificates: read_bounded(CERTIFICATES, MAX_CA_BYTES)?,
        })))
    }

    pub(super) async fn configure(&self, request: ConfigureNetwork) -> io::Result<bool> {
        let defaults = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            let native = request.native_dns.is_some() && Path::new(native::SERVERS).try_exists()?;
            let mut writes = Vec::new();
            let resolver = if native {
                let servers = crate::native_dns::servers(&defaults.resolver, &request)?;
                writes.push((native::SERVERS, servers.into_bytes(), 0o644));
                let local = ConfigureNetwork {
                    dns_servers: vec!["127.0.0.1".into()],
                    search_domains: request.search_domains.clone(),
                    ..Default::default()
                };
                resolver::resolv_conf("", &local)?
            } else {
                resolver::resolv_conf(&defaults.resolver, &request)?
            };
            let hosts = resolver::hosts_file(&defaults.hosts, &request)?;
            let original = read_bounded(DAEMON, MAX_CONFIG_BYTES)?;
            let config = resolver::daemon_config(&original, &request)?;
            let running = Path::new(super::READY_FILE).exists();
            let daemon_changed = config.is_some();

            writes.extend([
                (RESOLVER, resolver.into_bytes(), 0o644),
                (HOSTS, hosts.into_bytes(), 0o644),
            ]);
            if let Some(config) = config {
                writes.push((DAEMON, config, 0o600));
            }
            replace_all(&writes)?;
            if native {
                native::reload()?;
            }
            Ok(running && daemon_changed)
        })
        .await?
    }

    pub(super) async fn certificates(&self, request: InstallCertificates) -> io::Result<bool> {
        let defaults = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            let certificate_bundle =
                trust::bundle(&defaults.certificates, &request.certificates_der)?;
            let current = read_bounded(CERTIFICATES, MAX_CA_BYTES)?;
            if current == certificate_bundle {
                return Ok(false);
            }
            atomic_file(CERTIFICATES, &certificate_bundle, 0o644)?;
            Ok(Path::new(super::READY_FILE).exists())
        })
        .await?
    }
}

fn replace_all(writes: &[(&str, Vec<u8>, u32)]) -> io::Result<()> {
    let originals = writes
        .iter()
        .map(|(path, _, _)| match read_bounded(path, MAX_CONFIG_BYTES) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        })
        .collect::<io::Result<Vec<_>>>()?;
    for (index, (path, bytes, mode)) in writes.iter().enumerate() {
        if let Err(error) = atomic_file(path, bytes, *mode) {
            for ((previous, _, mode), original) in writes[..index].iter().zip(&originals).rev() {
                let result = match original {
                    Some(bytes) => atomic_file(previous, bytes, *mode),
                    None => fs::remove_file(previous),
                };
                if result.is_err() {
                    eprintln!("marsa-agent: guest network configuration rollback failed");
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

fn atomic_file(path: &str, contents: &[u8], mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let path = Path::new(path);
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("guest config must have a parent directory"))?;
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    staged.write_all(contents)?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn read_or_empty(path: &str) -> io::Result<Vec<u8>> {
    match read_bounded(path, MAX_CONFIG_BYTES) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        result => result,
    }
}

fn read_bounded(path: &str, limit: usize) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut contents)?;
    if contents.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "guest configuration exceeds its size limit",
        ));
    }
    Ok(contents)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
