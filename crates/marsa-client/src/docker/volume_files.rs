//! Stage a prepared volume download until the guest confirms the pinned selection.

use std::{io, path::Path, time::Duration};

use marsa_proto::guest::{
    VOLUME_READ_CHUNK, VOLUME_READ_LIMIT,
    v1::{
        PrepareVolumeRead, VolumeReadFailure, VolumeReadFormat, VolumeReadReply, VolumeReadRequest,
        volume_read_reply::Body as Reply, volume_read_request::Body as Input,
    },
    validate_volume_read,
};
use marsa_proto::host::VOLUME_FILES_API_MINOR;
use thiserror::Error;
use tokio::{fs::File, io::AsyncWriteExt as _, sync::mpsc, time::timeout};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Status, Streaming};

use crate::MarsaClient;

use super::{ContainerFileFormat, VolumeSummary, container_files::validate_tar};

const IDLE: Duration = Duration::from_secs(60);
const TRANSFER: Duration = Duration::from_secs(30 * 60);

/// Failure categories for a confined owner volume download.
#[derive(Debug, Error)]
pub enum VolumeDownloadError {
    /// The selected engine or volume cannot provide confined reads.
    #[error("a confined volume provider is unavailable")]
    Unsupported,
    /// The built-in engine is stopped.
    #[error("volume downloads require the built-in engine to be running")]
    Unavailable,
    /// The prepared volume or path no longer resolves to the same file.
    #[error("the volume selection changed")]
    Changed,
    /// The selected path was invalid or could not be confined.
    #[error("the volume path could not be read safely")]
    InvalidPath,
    /// The archive or file exceeds a bounded transfer limit.
    #[error("the volume download exceeds a size limit")]
    Limit,
    /// The stream was interrupted or invalid.
    #[error("the volume download did not complete")]
    Transfer,
    /// The selected destination could not be created.
    #[error("the volume download could not be saved")]
    Destination(#[source] io::Error),
}

/// A file or archive selected by the guest while its volume identity stays pinned.
pub struct VolumeDownload {
    outbound: mpsc::Sender<VolumeReadRequest>,
    inbound: Streaming<VolumeReadReply>,
    format: ContainerFileFormat,
}

impl MarsaClient {
    /// Probe the already-running built-in VM without activating it.
    pub async fn check_volume_provider(&self) -> Result<(), VolumeDownloadError> {
        let (_, mut stream) = self.volume_stream(Input::Probe(true)).await?;
        match next(&mut stream).await? {
            Reply::Capable(true) => Ok(()),
            Reply::Failure(error) => Err(guest_error(error)),
            _ => Err(VolumeDownloadError::Unsupported),
        }
    }

    /// Prepare a path inside the named volume belonging to this owner connection.
    ///
    /// Drop the returned value to cancel the read before choosing a destination.
    pub async fn prepare_volume_download(
        &self,
        volume: &VolumeSummary,
        path: String,
        prefer_single_file: bool,
    ) -> Result<VolumeDownload, VolumeDownloadError> {
        if volume.driver != "local" || volume.scope != "local" {
            return Err(VolumeDownloadError::Unsupported);
        }
        let selection = PrepareVolumeRead {
            name: volume.name.clone(),
            created_at: volume
                .created_at
                .clone()
                .ok_or(VolumeDownloadError::Unsupported)?,
            path,
            prefer_single_file,
        };
        validate_volume_read(&selection).map_err(|_| VolumeDownloadError::InvalidPath)?;
        let (outbound, mut inbound) = self.volume_stream(Input::Prepare(selection)).await?;
        let format = match next(&mut inbound).await? {
            Reply::Format(format) => match VolumeReadFormat::try_from(format) {
                Ok(VolumeReadFormat::Tar) => ContainerFileFormat::Tar,
                Ok(VolumeReadFormat::SingleFile) if prefer_single_file => {
                    ContainerFileFormat::SingleFile
                }
                _ => return Err(VolumeDownloadError::Transfer),
            },
            Reply::Failure(error) => return Err(guest_error(error)),
            _ => return Err(VolumeDownloadError::Transfer),
        };
        Ok(VolumeDownload {
            outbound,
            inbound,
            format,
        })
    }

    async fn volume_stream(
        &self,
        body: Input,
    ) -> Result<(mpsc::Sender<VolumeReadRequest>, Streaming<VolumeReadReply>), VolumeDownloadError>
    {
        if self.api_minor() < VOLUME_FILES_API_MINOR {
            return Err(VolumeDownloadError::Unsupported);
        }
        let (send, receive) = mpsc::channel(1);
        send.try_send(VolumeReadRequest { body: Some(body) })
            .map_err(|_| VolumeDownloadError::Transfer)?;
        let stream = timeout(
            IDLE,
            self.volume_files()
                .max_decoding_message_size(VOLUME_READ_CHUNK + 16)
                .read(ReceiverStream::new(receive)),
        )
        .await
        .map_err(|_| VolumeDownloadError::Transfer)?
        .map_err(status_error)?
        .into_inner();
        Ok((send, stream))
    }
}

impl VolumeDownload {
    /// The format selected by the guest from the saved preference and entry type.
    pub fn format(&self) -> ContainerFileFormat {
        self.format
    }

    /// Start the prepared transfer and publish only to the new chosen destination.
    pub async fn save(self, destination: &Path) -> Result<(), VolumeDownloadError> {
        let Self {
            outbound,
            mut inbound,
            format,
        } = self;
        let destination = destination.to_owned();
        let parent = destination
            .parent()
            .filter(|_| destination.is_absolute() && destination.file_name().is_some())
            .ok_or_else(|| VolumeDownloadError::Destination(io::ErrorKind::InvalidInput.into()))?
            .to_owned();
        let staging = tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(parent))
            .await
            .map_err(|_| VolumeDownloadError::Transfer)?
            .map_err(VolumeDownloadError::Destination)?;
        let (file, path) = staging.into_parts();
        let mut file = File::from_std(file);
        outbound
            .send(VolumeReadRequest {
                body: Some(Input::Start(true)),
            })
            .await
            .map_err(|_| VolumeDownloadError::Transfer)?;
        drop(outbound);
        let length = timeout(TRANSFER, async {
            let mut length = 0;
            loop {
                match next(&mut inbound).await? {
                    Reply::Data(chunk) if !chunk.is_empty() && chunk.len() <= VOLUME_READ_CHUNK => {
                        length += chunk.len() as u64;
                        if length > VOLUME_READ_LIMIT {
                            return Err(VolumeDownloadError::Limit);
                        }
                        file.write_all(&chunk)
                            .await
                            .map_err(VolumeDownloadError::Destination)?;
                    }
                    Reply::Complete(true) => break,
                    Reply::Failure(error) => return Err(guest_error(error)),
                    _ => return Err(VolumeDownloadError::Transfer),
                }
            }
            let extra = timeout(IDLE, inbound.message())
                .await
                .map_err(|_| VolumeDownloadError::Transfer)?
                .map_err(status_error)?;
            if extra.is_some() {
                return Err(VolumeDownloadError::Transfer);
            }
            file.sync_all()
                .await
                .map_err(VolumeDownloadError::Destination)?;
            Ok(length)
        })
        .await
        .map_err(|_| VolumeDownloadError::Transfer)??;
        let mut file = file.into_std().await;
        tokio::task::spawn_blocking(move || {
            if format == ContainerFileFormat::Tar {
                validate_tar(&mut file, length).map_err(|_| VolumeDownloadError::Transfer)?;
            }
            path.persist_noclobber(destination)
                .map_err(|error| VolumeDownloadError::Destination(error.error))
        })
        .await
        .map_err(|_| VolumeDownloadError::Transfer)?
    }
}

async fn next(stream: &mut Streaming<VolumeReadReply>) -> Result<Reply, VolumeDownloadError> {
    timeout(IDLE, stream.message())
        .await
        .map_err(|_| VolumeDownloadError::Transfer)?
        .map_err(status_error)?
        .and_then(|reply| reply.body)
        .ok_or(VolumeDownloadError::Transfer)
}

fn status_error(error: Status) -> VolumeDownloadError {
    match error.code() {
        Code::Unimplemented => VolumeDownloadError::Unsupported,
        Code::FailedPrecondition => VolumeDownloadError::Unavailable,
        Code::ResourceExhausted => VolumeDownloadError::Limit,
        Code::InvalidArgument => VolumeDownloadError::InvalidPath,
        _ => VolumeDownloadError::Transfer,
    }
}

fn guest_error(code: i32) -> VolumeDownloadError {
    match VolumeReadFailure::try_from(code) {
        Ok(VolumeReadFailure::Unsupported) => VolumeDownloadError::Unsupported,
        Ok(VolumeReadFailure::Changed) => VolumeDownloadError::Changed,
        Ok(VolumeReadFailure::InvalidPath) => VolumeDownloadError::InvalidPath,
        Ok(VolumeReadFailure::Limit) => VolumeDownloadError::Limit,
        _ => VolumeDownloadError::Transfer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper_util::rt::TokioIo;
    use marsa_proto::host::v1::{
        volume_files_client::VolumeFilesClient,
        volume_files_server::{VolumeFiles, VolumeFilesServer},
    };
    use tokio::net::{UnixListener, UnixStream};
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::{
        Request, Response,
        transport::{Endpoint, Server},
    };
    use tower::service_fn;

    struct Fixture {
        replies: Vec<Reply>,
        wait_for_drop: bool,
    }

    #[tonic::async_trait]
    impl VolumeFiles for Fixture {
        type ReadStream = ReceiverStream<Result<VolumeReadReply, Status>>;

        async fn read(
            &self,
            request: Request<Streaming<VolumeReadRequest>>,
        ) -> Result<Response<Self::ReadStream>, Status> {
            let mut requests = request.into_inner();
            let (sender, output) = mpsc::channel(1);
            let replies = self.replies.clone();
            let wait_for_drop = self.wait_for_drop;
            tokio::spawn(async move {
                assert_eq!(
                    requests.message().await.unwrap().unwrap().body,
                    Some(Input::Start(true))
                );
                for body in replies {
                    if sender
                        .send(Ok(VolumeReadReply { body: Some(body) }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                if wait_for_drop {
                    sender.closed().await;
                }
            });
            Ok(Response::new(ReceiverStream::new(output)))
        }
    }

    async fn prepared(
        replies: Vec<Reply>,
        wait_for_drop: bool,
    ) -> (VolumeDownload, tokio::task::JoinHandle<()>) {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let socket = directory.path().join("volume.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let _socket_directory = directory;
            Server::builder()
                .add_service(VolumeFilesServer::new(Fixture {
                    replies,
                    wait_for_drop,
                }))
                .serve_with_incoming(UnixListenerStream::new(listener))
                .await
                .unwrap();
        });
        let (outbound, incoming) = mpsc::channel(1);
        let channel = Endpoint::from_static("http://localhost")
            .connect_with_connector(service_fn(move |_| {
                let socket = socket.clone();
                async move { UnixStream::connect(socket).await.map(TokioIo::new) }
            }))
            .await
            .unwrap();
        let inbound = VolumeFilesClient::new(channel)
            .read(ReceiverStream::new(incoming))
            .await
            .unwrap()
            .into_inner();
        (
            VolumeDownload {
                outbound,
                inbound,
                format: ContainerFileFormat::SingleFile,
            },
            server,
        )
    }

    #[tokio::test]
    async fn volume_reads_require_completion_and_preserve_existing_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("selected.txt");
        let data = Reply::Data(b"approved bytes".to_vec());

        for replies in [
            vec![data.clone()],
            vec![
                data.clone(),
                Reply::Failure(VolumeReadFailure::Changed as i32),
            ],
            vec![data.clone(), Reply::Complete(true), data.clone()],
        ] {
            let (download, server) = prepared(replies, false).await;
            assert!(download.save(&destination).await.is_err());
            assert_eq!(directory.path().read_dir().unwrap().count(), 0);
            server.abort();
        }

        let (download, server) = prepared(vec![data.clone(), Reply::Complete(true)], false).await;
        download.save(&destination).await.unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"approved bytes");
        server.abort();

        let (download, server) = prepared(
            vec![Reply::Data(b"replacement".to_vec()), Reply::Complete(true)],
            false,
        )
        .await;
        assert!(matches!(
            download.save(&destination).await,
            Err(VolumeDownloadError::Destination(_))
        ));
        assert_eq!(std::fs::read(&destination).unwrap(), b"approved bytes");
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
        server.abort();
    }
    #[tokio::test]
    async fn cancelling_an_active_volume_transfer_removes_the_staged_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("selected.txt");
        let contents = b"unconfirmed bytes";
        let (download, server) = prepared(vec![Reply::Data(contents.to_vec())], true).await;
        let transfer = tokio::spawn({
            let destination = destination.clone();
            async move { download.save(&destination).await }
        });
        let staged = timeout(Duration::from_secs(5), async {
            loop {
                for file in directory.path().read_dir().unwrap() {
                    let file = file.unwrap().path();
                    if std::fs::read(&file).is_ok_and(|bytes| bytes == contents) {
                        return file;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the stream writes only to a temporary file before completion");
        assert_ne!(staged, destination);
        transfer.abort();
        assert!(transfer.await.unwrap_err().is_cancelled());
        assert_eq!(directory.path().read_dir().unwrap().count(), 0);
        server.abort();
    }
}
