//! Download a path from an explicitly selected container without unpacking paths on the host.

use std::{fmt::Write as _, io, path::Path, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::{BodyExt as _, Full};
use hyper::{HeaderMap, Method, body::Bytes, header};
use serde::Deserialize;
use tokio::{
    fs::File,
    io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _},
    time::timeout,
};

use super::{ContainerId, DockerClient, DockerError};

const IDLE: Duration = Duration::from_secs(60);
const TRANSFER: Duration = Duration::from_secs(30 * 60);
const LIMIT: u64 = 16 * 1024 * 1024 * 1024;
const MAX_PATH: usize = 4096;
// Go's os.FileMode serializes directory, symlink, device, pipe, socket and irregular bits.
const GO_FILE_TYPE_MASK: u32 = 0x8f28_0000;

/// A validated, absolute path in the selected Linux container.
#[derive(Clone)]
pub struct ContainerPath(String);

impl ContainerPath {
    /// Reject relative and ambiguous path components before forming an engine request.
    pub fn parse(path: impl Into<String>) -> Result<Self, DockerError> {
        let path = path.into();
        if !path.starts_with('/')
            || path.len() > MAX_PATH
            || path.chars().any(char::is_control)
            || path != "/"
                && path[1..]
                    .split('/')
                    .any(|component| matches!(component, "" | "." | ".."))
        {
            return Err(DockerError::ContainerFilePath);
        }
        Ok(Self(path))
    }

    fn route(&self, id: &ContainerId) -> String {
        let mut route = format!("/containers/{}/archive?path=", id.as_str());
        for byte in self.0.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                route.push(char::from(byte));
            } else {
                let _ = write!(route, "%{byte:02X}");
            }
        }
        route
    }
}

pub(super) fn valid_query(query: &str) -> bool {
    if query.len() > MAX_PATH * 3 {
        return false;
    }
    let mut bytes = query.bytes();
    let mut decoded = Vec::with_capacity(query.len());
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let Some(high) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            let Some(low) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            decoded.push((high * 16 + low) as u8);
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            decoded.push(byte);
        } else {
            return false;
        }
    }
    String::from_utf8(decoded)
        .ok()
        .is_some_and(|path| ContainerPath::parse(path).is_ok())
}

#[derive(Deserialize)]
struct PathStat {
    mode: u32,
    size: i64,
}

/// The file type the save dialog uses to suggest an appropriate extension.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainerFileFormat {
    /// Save the engine archive as a TAR file.
    Tar,
    /// Save the bytes of exactly one regular file.
    SingleFile,
}

impl PathStat {
    fn read(headers: &HeaderMap) -> Result<Self, DockerError> {
        let header = headers
            .get("x-docker-container-path-stat")
            .filter(|value| value.len() <= 8192)
            .ok_or_else(invalid_archive)?;
        let bytes = STANDARD
            .decode(header.as_bytes())
            .map_err(|_| invalid_archive())?;
        serde_json::from_slice(&bytes).map_err(|_| invalid_archive())
    }

    fn regular_file(&self) -> bool {
        self.mode & GO_FILE_TYPE_MASK == 0 && (0..=LIMIT as i64).contains(&self.size)
    }
}

impl DockerClient {
    /// Identify the output format without transferring the file data.
    pub async fn container_file_format(
        &self,
        id: &ContainerId,
        path: &ContainerPath,
        prefer_raw: bool,
    ) -> Result<ContainerFileFormat, DockerError> {
        let (headers, _) = self.request(Method::HEAD, &path.route(id)).await?;
        let stat = PathStat::read(&headers)?;
        Ok(if prefer_raw && stat.regular_file() {
            ContainerFileFormat::SingleFile
        } else {
            ContainerFileFormat::Tar
        })
    }

    /// Transfer the selected path to a new owner-readable file.
    ///
    /// Archive names are never used to write host files.
    pub async fn download_container_file(
        &self,
        id: &ContainerId,
        path: &ContainerPath,
        format: ContainerFileFormat,
        destination: &Path,
    ) -> Result<(), DockerError> {
        let directory = destination
            .parent()
            .filter(|_| destination.is_absolute() && destination.file_name().is_some())
            .ok_or_else(invalid_archive)?
            .to_owned();
        let destination = destination.to_owned();
        let buffer =
            tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(directory))
                .await
                .map_err(task_error)?
                .map_err(DockerError::ContainerArchive)?;
        let (file, staging) = buffer.into_parts();
        let mut file = File::from_std(file);
        let (headers, mut response) = self
            .request_body(
                Method::GET,
                &path.route(id),
                Full::new(Bytes::new()),
                HeaderMap::new(),
                IDLE,
            )
            .await?;
        if !headers
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value == "application/x-tar")
        {
            return Err(invalid_archive());
        }
        let stat = PathStat::read(&headers)?;
        if matches!(format, ContainerFileFormat::SingleFile) && !stat.regular_file() {
            return Err(invalid_archive());
        }
        timeout(TRANSFER, async {
            let mut length = 0;
            while let Some(frame) = timeout(IDLE, response.body.frame())
                .await
                .map_err(|_| DockerError::Timeout)?
                .transpose()?
            {
                if let Ok(chunk) = frame.into_data() {
                    length += chunk.len() as u64;
                    if length > LIMIT {
                        return Err(invalid_archive());
                    }
                    file.write_all(&chunk)
                        .await
                        .map_err(DockerError::ContainerArchive)?;
                }
            }
            if length < 1024 || length % 512 != 0 {
                return Err(invalid_archive());
            }
            file.seek(io::SeekFrom::End(-1024))
                .await
                .map_err(DockerError::ContainerArchive)?;
            let mut end_marker = [0; 1024];
            file.read_exact(&mut end_marker)
                .await
                .map_err(DockerError::ContainerArchive)?;
            if end_marker.iter().any(|&byte| byte != 0) {
                return Err(invalid_archive());
            }
            file.sync_all().await.map_err(DockerError::ContainerArchive)
        })
        .await
        .map_err(|_| DockerError::Timeout)??;
        drop(file);
        tokio::task::spawn_blocking(move || match format {
            ContainerFileFormat::Tar => staging
                .persist_noclobber(destination)
                .map_err(|error| DockerError::ContainerArchive(error.error)),
            ContainerFileFormat::SingleFile => {
                publish_single_file(&staging, &destination, stat.size as u64)
                    .map_err(DockerError::ContainerArchive)
            }
        })
        .await
        .map_err(task_error)?
    }
}

fn publish_single_file(staging: &Path, destination: &Path, expected: u64) -> io::Result<()> {
    let directory = destination.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let mut output = tempfile::NamedTempFile::new_in(directory)?;
    let mut archive = tar::Archive::new(std::fs::File::open(staging)?);
    // Raw iteration avoids allocating data from untrusted long-name or PAX records.
    let mut entries = archive.entries()?.raw(true);
    let mut entry = entries.next().ok_or(io::ErrorKind::InvalidData)??;
    if !entry.header().entry_type().is_file()
        || entry.size() != expected
        || io::copy(&mut entry, output.as_file_mut())? != expected
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if entries.next().is_some() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    output.as_file().sync_all()?;
    output
        .persist_noclobber(destination)
        .map_err(|error| error.error)?;
    Ok(())
}

fn invalid_archive() -> DockerError {
    DockerError::ContainerArchive(io::ErrorKind::InvalidData.into())
}

fn task_error(error: tokio::task::JoinError) -> DockerError {
    DockerError::ContainerArchive(io::Error::other(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_remain_literal_and_cannot_supply_extra_parameters() {
        assert!(ContainerPath::parse("/var/log/a b.txt").is_ok());
        assert!(valid_query("%2Fvar%2Flog%2Fa%20b.txt"));
        assert!(!valid_query("%2Fetc%2F..%2Froot"));
        assert!(!valid_query("%2Fetc&another=true"));
        assert!(!valid_query("etc"));
        assert!(ContainerPath::parse("/").is_ok());
    }

    #[test]
    fn raw_download_accepts_only_one_regular_entry_and_keeps_original_destination() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.tar");
        let output = root.path().join("result");
        let content: &[u8] = b"file contents";
        let mut archive = tar::Builder::new(std::fs::File::create(&source).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        archive.append_data(&mut header, "file", content).unwrap();
        archive.finish().unwrap();
        publish_single_file(&source, &output, content.len() as u64).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), content);
        assert!(publish_single_file(&source, &output, content.len() as u64).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), content);
        assert!(!root.path().join("file").exists());
    }
}
