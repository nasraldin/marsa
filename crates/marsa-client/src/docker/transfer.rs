//! Engine-scoped image archives, registry push, and network membership changes.

mod auth;
#[cfg(test)]
mod tests;

use std::{
    io,
    net::Ipv6Addr,
    path::Path,
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};

use http_body_util::{BodyExt as _, Full};
use hyper::{
    HeaderMap, Method,
    body::{Body, Bytes, Frame, SizeHint},
    header::{self, HeaderValue},
};
use serde::Deserialize;
use serde_json::json;
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncRead, AsyncWriteExt as _, ReadBuf},
    time::timeout,
};

use super::{
    ContainerId, DockerClient, DockerError, ImageId, ImageReference, images::valid_tag,
    stream::ResponseBody,
};

const LOAD_PATH: &str = "/images/load?quiet=true";
const ARCHIVE_TYPE: &str = "application/x-tar";
const CHUNK_SIZE: usize = 64 * 1024;
const MAX_ARCHIVE_SIZE: u64 = 128 * 1024 * 1024 * 1024;
const TRANSFER_DEADLINE: Duration = Duration::from_secs(30 * 60);
const STREAM_IDLE: Duration = Duration::from_secs(120);

/// A complete Docker network ID used in fixed HTTP routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkId(String);

impl NetworkId {
    /// Parse the full 64-character lowercase hexadecimal network ID.
    pub fn parse(value: impl Into<String>) -> Result<Self, DockerError> {
        let value = value.into();
        if !valid_network_id(&value) {
            return Err(DockerError::InvalidResourceName);
        }
        Ok(Self(value))
    }

    /// Return the complete validated identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn valid_network_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

struct PushTarget<'a> {
    repository: &'a str,
    registry: &'a str,
    tag: &'a str,
}

impl<'a> PushTarget<'a> {
    fn parse(image: &'a ImageReference) -> Result<Self, DockerError> {
        let (repository, tag) = image
            .as_str()
            .rsplit_once(':')
            .ok_or(DockerError::InvalidResourceName)?;
        Self::from_parts(repository, tag).ok_or(DockerError::InvalidResourceName)
    }

    fn from_parts(repository: &'a str, tag: &'a str) -> Option<Self> {
        if repository.len() > 255 || !valid_tag(tag) || valid_network_id(repository) {
            return None;
        }

        // Match Docker's familiar-name rules before selecting credentials. In a
        // multi-component name, a dotted host, a port, localhost, or uppercase
        // in the first component denotes an explicit registry.
        let (registry, name) = match repository.split_once('/') {
            Some((first, name))
                if first == "localhost"
                    || first.contains(['.', ':'])
                    || first.bytes().any(|b| b.is_ascii_uppercase()) =>
            {
                if !valid_registry_host(first) {
                    return None;
                }
                let registry = match first {
                    "docker.io" | "index.docker.io" => auth::DOCKER_HUB,
                    _ => first,
                };
                (registry, name)
            }
            _ => (auth::DOCKER_HUB, repository),
        };
        if !name.split('/').all(valid_repository_component) {
            return None;
        }

        Some(Self {
            repository,
            registry,
            tag,
        })
    }

    fn path(&self) -> String {
        format!("/images/{}/push?tag={}", self.repository, self.tag)
    }
}

fn valid_repository_component(component: &str) -> bool {
    let atom = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let mut rest = component.as_bytes();
    loop {
        let atom_len = rest.iter().take_while(|byte| atom(byte)).count();
        if atom_len == 0 {
            return false;
        }
        rest = &rest[atom_len..];
        rest = match rest {
            [] => return true,
            [b'_', b'_', rest @ ..] | [b'_' | b'.', rest @ ..] => rest,
            [b'-', ..] => &rest[rest.iter().take_while(|&&b| b == b'-').count()..],
            _ => return false,
        };
    }
}

fn valid_registry_host(input: &str) -> bool {
    let port = |port: &str| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
    if let Some(ip) = input.strip_prefix('[') {
        return ip.split_once(']').is_some_and(|(ip, suffix)| {
            ip.parse::<Ipv6Addr>().is_ok()
                && (suffix.is_empty() || suffix.strip_prefix(':').is_some_and(port))
        });
    }
    let host = match input.split_once(':') {
        Some((host, suffix)) if port(suffix) => host,
        Some(_) => return false,
        None => input,
    };
    host.split('.').all(|label| {
        label
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

pub(super) fn external_transfer_route(method: &Method, path: &str, has_body: bool) -> bool {
    match (method, has_body) {
        (&Method::GET, false) => path
            .strip_prefix("/images/")
            .and_then(|name| name.strip_suffix("/get"))
            .is_some_and(|name| ImageId::parse(name).is_ok()),
        (&Method::POST, true) => {
            path == LOAD_PATH
                || path
                    .strip_prefix("/networks/")
                    .and_then(|network| network.split_once('/'))
                    .is_some_and(|(id, verb)| {
                        valid_network_id(id) && matches!(verb, "connect" | "disconnect")
                    })
        }
        (&Method::POST, false) => path
            .strip_prefix("/images/")
            .and_then(|path| path.split_once("/push?tag="))
            .is_some_and(|(repo, tag)| PushTarget::from_parts(repo, tag).is_some()),
        _ => false,
    }
}

impl DockerClient {
    /// Push exactly one existing repository tag through the selected, authenticated engine.
    ///
    /// Registry credentials are resolved for that repository from local Docker configuration.
    /// The engine receives only that registry's authorization.
    pub async fn push_image(&self, image: &ImageReference) -> Result<(), DockerError> {
        let target = PushTarget::parse(image)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-registry-auth",
            auth::registry_header(target.registry).await?,
        );
        let (_, mut response) = self
            .request_body(
                Method::POST,
                &target.path(),
                Full::new(Bytes::new()),
                headers,
                STREAM_IDLE,
            )
            .await?;
        read_transfer_result(&mut response, || DockerError::ImagePush).await
    }

    /// Save the selected immutable image to a new archive at a user-selected absolute path.
    ///
    /// Docker's export by ID omits repository tag metadata. The archive is published only
    /// after the complete response is written, and an existing destination is preserved.
    pub async fn export_image(
        &self,
        image: &ImageId,
        destination: &Path,
    ) -> Result<(), DockerError> {
        self.export_image_with_limit(image, destination, MAX_ARCHIVE_SIZE)
            .await
    }

    pub(super) async fn export_image_with_limit(
        &self,
        image: &ImageId,
        destination: &Path,
        size_limit: u64,
    ) -> Result<(), DockerError> {
        let directory = destination
            .parent()
            .filter(|_| destination.is_absolute() && destination.file_name().is_some())
            .ok_or_else(invalid_archive)?;
        let directory = directory.to_owned();
        let temp = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .prefix(".marsa-image-")
                .tempfile_in(directory)
        })
        .await
        .map_err(archive_task)?
        .map_err(DockerError::ImageArchive)?;
        let (file, temporary) = temp.into_parts();
        let mut file = File::from_std(file);

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static(ARCHIVE_TYPE));
        let (headers, mut source) = self
            .request_body(
                Method::GET,
                &format!("/images/{}/get", image.as_str()),
                Full::new(Bytes::new()),
                headers,
                STREAM_IDLE,
            )
            .await?;
        if headers.get(header::CONTENT_TYPE) != Some(&HeaderValue::from_static(ARCHIVE_TYPE))
            || headers
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|length| length > size_limit)
        {
            return Err(invalid_archive());
        }
        let mut length = 0;
        timeout(TRANSFER_DEADLINE, async {
            while let Some(frame) = timeout(STREAM_IDLE, source.body.frame())
                .await
                .map_err(|_| DockerError::Timeout)?
                .transpose()?
            {
                if let Ok(chunk) = frame.into_data() {
                    length += chunk.len() as u64;
                    if length > size_limit {
                        return Err(invalid_archive());
                    }
                    file.write_all(&chunk)
                        .await
                        .map_err(DockerError::ImageArchive)?;
                }
            }
            if length == 0 {
                return Err(invalid_archive());
            }
            file.sync_all().await.map_err(DockerError::ImageArchive)
        })
        .await
        .map_err(|_| DockerError::Timeout)??;

        let destination = destination.to_owned();
        tokio::task::spawn_blocking(move || temporary.persist_noclobber(destination))
            .await
            .map_err(archive_task)?
            .map_err(|error| DockerError::ImageArchive(error.error))
    }

    /// Send a user-selected local Docker archive to this engine's image-load endpoint.
    ///
    /// This passes archive bytes through without extracting them on the host.
    pub async fn import_image(&self, archive: &Path) -> Result<(), DockerError> {
        if !archive.is_absolute() {
            return Err(invalid_archive());
        }
        let file = open_regular_file(archive)
            .await
            .map_err(DockerError::ImageArchive)?;
        let length = file
            .metadata()
            .await
            .map_err(DockerError::ImageArchive)?
            .len();
        if !(1..=MAX_ARCHIVE_SIZE).contains(&length) {
            return Err(invalid_archive());
        }
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ARCHIVE_TYPE));
        timeout(TRANSFER_DEADLINE, async {
            let (_, mut response) = self
                .request_body(
                    Method::POST,
                    LOAD_PATH,
                    FileBody::new(file, length),
                    headers,
                    TRANSFER_DEADLINE,
                )
                .await?;
            read_transfer_result(&mut response, || DockerError::ImageLoad).await
        })
        .await
        .map_err(|_| DockerError::Timeout)?
    }

    /// Connect a container and network known to this engine by their complete IDs.
    pub async fn connect_network(
        &self,
        network: &NetworkId,
        container: &ContainerId,
    ) -> Result<(), DockerError> {
        self.request_payload(
            Method::POST,
            &format!("/networks/{}/connect", network.as_str()),
            serde_json::to_vec(&json!({"Container": container.as_str()}))?.into(),
        )
        .await?;
        Ok(())
    }

    /// Disconnect a container from its network without forcing endpoint removal.
    pub async fn disconnect_network(
        &self,
        network: &NetworkId,
        container: &ContainerId,
    ) -> Result<(), DockerError> {
        self.request_payload(
            Method::POST,
            &format!("/networks/{}/disconnect", network.as_str()),
            serde_json::to_vec(&json!({"Container": container.as_str(), "Force": false}))?.into(),
        )
        .await?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct TransferRecord {
    error: Option<serde::de::IgnoredAny>,
    #[serde(rename = "errorDetail")]
    error_detail: Option<serde::de::IgnoredAny>,
}

async fn read_transfer_result(
    response: &mut ResponseBody,
    error: impl FnOnce() -> DockerError,
) -> Result<(), DockerError> {
    let mut line = Vec::new();
    timeout(TRANSFER_DEADLINE, async {
        loop {
            match timeout(STREAM_IDLE, response.json::<TransferRecord>(&mut line)).await {
                Ok(Ok(Some(record))) if record.error.is_none() && record.error_detail.is_none() => {
                }
                Ok(Ok(None)) => return Ok(()),
                Ok(Ok(Some(_)) | Err(DockerError::Json(_))) => return Err(error()),
                Ok(Err(other)) => return Err(other),
                Err(_) => return Err(DockerError::Timeout),
            }
        }
    })
    .await
    .map_err(|_| DockerError::Timeout)?
}

fn invalid_archive() -> DockerError {
    DockerError::ImageArchive(io::ErrorKind::InvalidData.into())
}

fn archive_task(error: tokio::task::JoinError) -> DockerError {
    DockerError::ImageArchive(io::Error::other(error))
}

async fn open_regular_file(path: &Path) -> io::Result<File> {
    // O_NONBLOCK prevents opening a FIFO from hanging before metadata can reject it.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
        .await?;
    if !file.metadata().await?.is_file() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(file)
}

struct FileBody {
    file: File,
    remaining: u64,
    chunk: Vec<u8>,
}

impl FileBody {
    fn new(file: File, length: u64) -> Self {
        Self {
            file,
            remaining: length,
            chunk: vec![0; CHUNK_SIZE],
        }
    }
}

impl Body for FileBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        let body = self.get_mut();
        if body.remaining == 0 {
            return Poll::Ready(None);
        }
        let size = body.remaining.min(CHUNK_SIZE as u64) as usize;
        let mut pending = ReadBuf::new(&mut body.chunk[..size]);
        if let Err(error) = ready!(Pin::new(&mut body.file).poll_read(context, &mut pending)) {
            body.remaining = 0;
            return Poll::Ready(Some(Err(error)));
        }
        let data = pending.filled();
        if data.is_empty() {
            body.remaining = 0;
            return Poll::Ready(Some(Err(io::ErrorKind::UnexpectedEof.into())));
        }
        body.remaining -= data.len() as u64;
        Poll::Ready(Some(Ok(Frame::data(Bytes::copy_from_slice(data)))))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }

    fn is_end_stream(&self) -> bool {
        self.remaining == 0
    }
}
