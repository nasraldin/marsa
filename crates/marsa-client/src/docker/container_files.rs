//! Download a path from an explicitly selected container without unpacking paths on the host.

use std::{fmt::Write as _, io, ops::Range, path::Path, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::{BodyExt as _, Full};
use hyper::{HeaderMap, Method, body::Bytes, header};
use serde::Deserialize;
use tokio::{fs::File, io::AsyncWriteExt as _, time::timeout};

use super::{ContainerId, DockerClient, DockerError};

const IDLE: Duration = Duration::from_secs(60);
const TRANSFER: Duration = Duration::from_secs(30 * 60);
const LIMIT: u64 = 16 * 1024 * 1024 * 1024;
const MAX_PATH: usize = 4096;
const TAR_BLOCK: u64 = 512;
// Go's archive/tar bounds extended headers in Docker's archive writer.
const MAX_PAX_HEADER: u64 = 1 << 20;
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
        let length = timeout(TRANSFER, async {
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
            if length < TAR_BLOCK * 2 || length % TAR_BLOCK != 0 {
                return Err(invalid_archive());
            }
            file.sync_all()
                .await
                .map_err(DockerError::ContainerArchive)?;
            Ok(length)
        })
        .await
        .map_err(|_| DockerError::Timeout)??;
        let mut archive = file.into_std().await;
        tokio::task::spawn_blocking(move || match format {
            ContainerFileFormat::Tar => {
                validate_tar(&mut archive, length).map_err(DockerError::ContainerArchive)?;
                staging
                    .persist_noclobber(destination)
                    .map_err(|error| DockerError::ContainerArchive(error.error))
            }
            ContainerFileFormat::SingleFile => {
                publish_single_file(&mut archive, &destination, stat.size as u64)
                    .map_err(DockerError::ContainerArchive)
            }
        })
        .await
        .map_err(task_error)?
    }
}

// Verify that the end markers follow complete entries. File data can itself
// end in zero blocks, so the final bytes alone cannot prove completeness.
pub(super) fn validate_tar(
    file: &mut std::fs::File,
    length: u64,
) -> io::Result<Option<Range<u64>>> {
    use std::io::{Read as _, Seek as _};

    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let last_marker_offset = length.checked_sub(TAR_BLOCK * 2).ok_or_else(invalid)?;
    let mut offset = 0;
    let mut size_override = None;
    let mut pending_metadata = false;
    let mut seen_entry = false;
    let mut single_file = None;

    while offset <= last_marker_offset {
        file.seek(io::SeekFrom::Start(offset))?;
        let mut header = tar::Header::new_old();
        file.read_exact(header.as_mut_bytes())?;
        if header.as_bytes().iter().all(|&byte| byte == 0) {
            if pending_metadata {
                return Err(invalid());
            }
            let mut padding = [0; 8192];
            let mut remaining = length - offset - TAR_BLOCK;
            while remaining != 0 {
                let count = remaining.min(padding.len() as u64) as usize;
                file.read_exact(&mut padding[..count])?;
                if padding[..count].iter().any(|&byte| byte != 0) {
                    return Err(invalid());
                }
                remaining -= count as u64;
            }
            return Ok(single_file);
        }

        let checksum = header.cksum()?;
        header.set_cksum();
        if header.cksum()? != checksum {
            return Err(invalid());
        }
        let kind = header.entry_type();
        let local_metadata =
            kind.is_pax_local_extensions() || kind.is_gnu_longname() || kind.is_gnu_longlink();
        let metadata = local_metadata || kind.is_pax_global_extensions();
        let header_size = header.entry_size()?;
        let size = if metadata {
            header_size
        } else {
            size_override.take().unwrap_or(header_size)
        };
        pending_metadata = local_metadata;

        if kind.is_gnu_sparse() {
            let mut extended = header.as_gnu().ok_or_else(invalid)?.is_extended();
            let mut sparse = tar::GnuExtSparseHeader::new();
            while extended {
                file.read_exact(sparse.as_mut_bytes())?;
                extended = sparse.is_extended();
            }
        }

        let payload = file.stream_position()?;
        let available = last_marker_offset
            .checked_sub(payload)
            .ok_or_else(invalid)?;
        if size > available {
            return Err(invalid());
        }
        if kind.is_pax_local_extensions() {
            if size > MAX_PAX_HEADER {
                return Err(invalid());
            }
            let mut data = vec![0; size as usize];
            file.read_exact(&mut data)?;
            size_override = pax_entry_size(&data).ok_or_else(invalid)?;
        }
        if !metadata {
            single_file = (!seen_entry && kind.is_file()).then_some(payload..payload + size);
            seen_entry = true;
        }
        offset = payload + size.next_multiple_of(TAR_BLOCK);
    }
    Err(invalid())
}

// PAX record lengths include the length field itself. Use them to skip
// arbitrary metadata values, including values with embedded newlines.
fn pax_entry_size(mut data: &[u8]) -> Option<Option<u64>> {
    let mut size = None;
    while !data.is_empty() {
        let separator = data.iter().position(|&byte| byte == b' ')?;
        let length: usize = std::str::from_utf8(&data[..separator]).ok()?.parse().ok()?;
        let (record, remaining) = data.split_at_checked(length)?;
        let field = record.get(separator + 1..)?.strip_suffix(b"\n")?;
        if let Some(value) = field.strip_prefix(b"size=") {
            size = Some(std::str::from_utf8(value).ok()?.parse().ok()?);
        }
        data = remaining;
    }
    Some(size)
}

fn publish_single_file(
    archive: &mut std::fs::File,
    destination: &Path,
    expected: u64,
) -> io::Result<()> {
    use std::io::{Read as _, Seek as _};

    let length = archive.metadata()?.len();
    let payload = validate_tar(archive, length)?
        .filter(|range| range.end - range.start == expected)
        .ok_or(io::ErrorKind::InvalidData)?;
    archive.seek(io::SeekFrom::Start(payload.start))?;

    let directory = destination.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let mut output = tempfile::NamedTempFile::new_in(directory)?;
    if io::copy(&mut archive.take(expected), output.as_file_mut())? != expected {
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

    #[tokio::test]
    async fn a_zero_filled_partial_entry_does_not_publish_a_tar() {
        use std::os::unix::fs::PermissionsExt as _;
        use tokio::{io::AsyncReadExt as _, net::UnixListener};

        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let socket = root.path().join("docker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let destination = root.path().join("download.tar");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            let mut header = tar::Header::new_ustar();
            header.set_path("payload.bin").unwrap();
            header.set_mode(0o644);
            header.set_size(4096);
            header.set_cksum();
            let stat = STANDARD.encode(br#"{"mode":420,"size":4096}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: 2048\r\nX-Docker-Container-Path-Stat: {stat}\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&[0; 1536]).await.unwrap();
        });

        let id = ContainerId::parse("a".repeat(64)).unwrap();
        let path = ContainerPath::parse("/payload.bin").unwrap();
        let result = DockerClient::at(socket)
            .download_container_file(&id, &path, ContainerFileFormat::Tar, &destination)
            .await;
        server.await.unwrap();
        assert!(result.is_err(), "a partial regular file cannot end a TAR");
        assert!(!destination.exists());
        assert_eq!(root.path().read_dir().unwrap().count(), 1);
    }

    #[test]
    fn archive_validation_follows_extended_entry_lengths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("download.tar");
        let mut archive = tar::Builder::new(Vec::new());
        archive
            .append_pax_extensions([
                ("path", b"file\nwith newline".as_slice()),
                ("size", b"1024".as_slice()),
            ])
            .unwrap();
        let mut bytes = archive.into_inner().unwrap();
        bytes.truncate(bytes.len() - 1024);
        let mut header = tar::Header::new_ustar();
        header.set_path("payload.bin").unwrap();
        header.set_mode(0o644);
        header.set_size(0);
        header.set_cksum();
        bytes.extend_from_slice(header.as_bytes());
        // The PAX size supplies the payload boundary when it does not fit
        // (and is omitted from) the main TAR header.
        bytes.extend_from_slice(&[0; 1024 + 1024]);

        std::fs::write(&path, &bytes).unwrap();
        validate_tar(&mut std::fs::File::open(&path).unwrap(), bytes.len() as u64).unwrap();
        bytes.truncate(bytes.len() - 512);
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            validate_tar(&mut std::fs::File::open(&path).unwrap(), bytes.len() as u64).is_err()
        );
    }

    #[test]
    fn raw_download_accepts_a_regular_file_with_a_pax_path() {
        let root = tempfile::tempdir().unwrap();
        let archive_path = root.path().join("download.tar");
        let output = root.path().join("payload.bin");
        let name = format!("fixture-{}.bin", "a".repeat(120));
        let contents = b"\x00\xfffile with a long name";
        let mut archive = tar::Builder::new(std::fs::File::create(&archive_path).unwrap());
        archive
            .append_pax_extensions([("path", name.as_bytes())])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        archive
            .append_data(&mut header, "payload.bin", &contents[..])
            .unwrap();
        archive.finish().unwrap();

        publish_single_file(
            &mut std::fs::File::open(archive_path).unwrap(),
            &output,
            contents.len() as u64,
        )
        .unwrap();
        assert_eq!(std::fs::read(output).unwrap(), contents);
        assert!(!root.path().join(name).exists());
    }

    #[test]
    fn raw_download_refuses_an_archive_with_multiple_payloads() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("download.tar");
        let output = root.path().join("payload.bin");
        let mut archive = tar::Builder::new(std::fs::File::create(&path).unwrap());
        for name in ["one", "two"] {
            let mut header = tar::Header::new_ustar();
            header.set_size(1);
            header.set_mode(0o644);
            archive
                .append_data(&mut header, name, b"x".as_slice())
                .unwrap();
        }
        archive.finish().unwrap();

        assert!(publish_single_file(&mut std::fs::File::open(path).unwrap(), &output, 1).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn raw_download_refuses_concatenated_archives() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("download.tar");
        let output = root.path().join("payload.bin");
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_ustar();
        header.set_size(1);
        header.set_mode(0o644);
        archive
            .append_data(&mut header, "one", b"x".as_slice())
            .unwrap();
        let bytes = archive.into_inner().unwrap();
        std::fs::write(&path, bytes.repeat(2)).unwrap();

        assert!(publish_single_file(&mut std::fs::File::open(path).unwrap(), &output, 1).is_err());
        assert!(!output.exists());
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
        publish_single_file(
            &mut std::fs::File::open(&source).unwrap(),
            &output,
            content.len() as u64,
        )
        .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), content);
        assert!(
            publish_single_file(
                &mut std::fs::File::open(&source).unwrap(),
                &output,
                content.len() as u64,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&output).unwrap(), content);
        assert!(!root.path().join("file").exists());
    }
}
