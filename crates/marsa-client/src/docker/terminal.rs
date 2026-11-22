//! Interactive terminal attachments over Docker's validated HTTP upgrade.

use http_body_util::Full;
use hyper::{
    Method, Request, StatusCode,
    body::Bytes,
    client::conn::http1,
    header,
    upgrade::{self, Upgraded},
};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, split},
    time::timeout,
};

use crate::runtime::{Receiver, Sender};

use super::{ContainerId, DockerClient, DockerError, RAW, REQUEST_TIMEOUT, stream::ConnectionTask};

/// Maximum bytes accepted from a single terminal input action.
pub const MAX_TERMINAL_INPUT: usize = 4096;
const OUTPUT_CHUNK: usize = 8192;
const SHELL_SCRIPT: &str = "for shell in /bin/bash /bin/zsh /bin/ash; do if [ -x \"$shell\" ]; then exec \"$shell\" -i; fi; done; exec /bin/sh -i";

/// A validated native terminal size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    columns: u16,
    rows: u16,
}

impl TerminalSize {
    /// Construct a nonzero size within the supported desktop bounds.
    pub fn new(columns: u16, rows: u16) -> Result<Self, DockerError> {
        if !(1..=1024).contains(&columns) || !(1..=1024).contains(&rows) {
            return Err(DockerError::TerminalSize);
        }
        Ok(Self { columns, rows })
    }

    fn docker(self) -> [u16; 2] {
        [self.rows, self.columns]
    }
}

/// Input accepted by an active terminal attachment.
pub enum ExecControl {
    /// Raw terminal key or paste bytes.
    Input(Vec<u8>),
    /// Adjust the container terminal size.
    Resize(TerminalSize),
}

/// A message from the interactive terminal attachment.
#[derive(Debug, PartialEq, Eq)]
pub enum ExecOutput {
    /// The engine established the authenticated TTY attachment.
    Opened,
    /// Terminal bytes, which may include ANSI sequences or partial UTF-8.
    Bytes(Vec<u8>),
}

#[derive(Deserialize)]
struct ExecCreated {
    #[serde(rename = "Id")]
    id: ContainerId,
}

/// The authenticated bidirectional session. Dropping it closes the attachment.
pub struct ExecSession {
    docker: DockerClient,
    id: ContainerId,
    stream: TokioIo<Upgraded>,
}

impl DockerClient {
    /// Open an interactive shell for one container, using the supplied terminal size.
    ///
    /// The fixed bootstrap prefers installed login shells and falls back to
    /// the container's POSIX shell. A minimal image without a shell returns
    /// the engine's output and closes the attachment.
    pub async fn start_terminal(
        &self,
        id: &ContainerId,
        size: TerminalSize,
    ) -> Result<ExecSession, DockerError> {
        let path = format!("/containers/{}/exec", id.as_str());
        let created: ExecCreated = self
            .post_json(
                &path,
                json!({
                    "AttachStdin": true,
                    "AttachStdout": true,
                    "AttachStderr": true,
                    "Tty": true,
                    "ConsoleSize": size.docker(),
                    "Cmd": ["/bin/sh", "-c", SHELL_SCRIPT],
                }),
            )
            .await?;
        let path = format!("/exec/{}/start", created.id.as_str());
        let body = serde_json::to_vec(&json!({
            "Detach": false,
            "Tty": true,
            "ConsoleSize": size.docker(),
        }))?;
        let stream = timeout(REQUEST_TIMEOUT, async {
            let socket = self.request_socket(&Method::POST, &path, true).await?;
            let (mut sender, connection) = http1::Builder::new()
                .max_buf_size(16 * 1024)
                .handshake(TokioIo::new(socket))
                .await?;
            let _connection =
                ConnectionTask::new(tokio::spawn(connection.with_upgrades()).abort_handle());
            let request = Request::builder()
                .method(Method::POST)
                .uri(self.request_uri(&path))
                .header(header::HOST, "docker")
                .header(header::CONNECTION, "Upgrade")
                .header(header::UPGRADE, "tcp")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(body)))?;
            let response = sender.send_request(request).await?;
            if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                return Err(DockerError::Status(response.status()));
            }
            let negotiated_tcp = response
                .headers()
                .get(header::UPGRADE)
                .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"tcp"));
            let raw_tty = response
                .headers()
                .get(header::CONTENT_TYPE)
                .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(RAW.as_bytes()));
            if !negotiated_tcp || !raw_tty {
                return Err(DockerError::TerminalUpgrade);
            }
            let upgraded = upgrade::on(response).await?;
            Ok(TokioIo::new(upgraded))
        })
        .await
        .map_err(|_| DockerError::Timeout)??;
        Ok(ExecSession {
            docker: self.clone(),
            id: created.id,
            stream,
        })
    }
}

impl ExecSession {
    /// Forward input and output independently until either side closes.
    ///
    /// Both data channels are bounded by their callers. Cancelling this
    /// future drops both halves of the upgraded socket and stops forwarding.
    pub async fn drive(
        self,
        mut input: Receiver<ExecControl>,
        output: &Sender<Result<ExecOutput, DockerError>>,
    ) -> Result<(), DockerError> {
        let Self { docker, id, stream } = self;
        let (mut reader, mut writer) = split(stream);
        if !output.send(Ok(ExecOutput::Opened)).await {
            return Ok(());
        }
        let read = async {
            let mut buffer = [0; OUTPUT_CHUNK];
            loop {
                let count = reader
                    .read(&mut buffer)
                    .await
                    .map_err(DockerError::TerminalIo)?;
                if count == 0
                    || !output
                        .send(Ok(ExecOutput::Bytes(buffer[..count].to_vec())))
                        .await
                {
                    return Ok(());
                }
            }
        };
        let write = async {
            while let Some(action) = input.next().await {
                match action {
                    ExecControl::Input(bytes) => {
                        if bytes.len() > MAX_TERMINAL_INPUT {
                            return Err(DockerError::RecordLimit);
                        }
                        writer
                            .write_all(&bytes)
                            .await
                            .map_err(DockerError::TerminalIo)?;
                    }
                    ExecControl::Resize(size) => {
                        let path = format!(
                            "/exec/{}/resize?h={}&w={}",
                            id.as_str(),
                            size.rows,
                            size.columns
                        );
                        // A resize may race the initial process start or its
                        // exit; the TTY stream reports the authoritative end.
                        let _ = docker.request(Method::POST, &path).await;
                    }
                }
            }
            Ok(())
        };
        tokio::select! {
            result = read => result,
            result = write => result,
        }
    }
}
