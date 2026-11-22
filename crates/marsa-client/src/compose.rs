//! Scoped Compose processes using a deliberately selected local project file.

use std::{io, path::PathBuf, process::Stdio};

use nix::unistd::geteuid;
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout, Command},
};

use crate::{ClientError, docker_socket_path, platform};

const MAX_OUTPUT_LINE: usize = 64 * 1024;

/// Lifecycle operation executed through the bundled Compose binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeOperation {
    /// Start the project in detached mode.
    Up,
    /// Stop and remove the project while keeping its volumes.
    Down,
    /// Restart existing project services.
    Restart,
}

/// One user-approved Compose operation and the local YAML file selected for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeCommand {
    file: PathBuf,
    project: String,
    operation: ComposeOperation,
    remove_volumes: bool,
}

impl ComposeCommand {
    /// Record a local YAML file deliberately selected by the owner.
    ///
    /// Do not pass paths supplied by container labels or a remote engine.
    pub fn for_selected_file(
        file: PathBuf,
        project: impl Into<String>,
        operation: ComposeOperation,
    ) -> Result<Self, ComposeError> {
        let project = project.into();
        if !file.is_absolute()
            || !file
                .extension()
                .and_then(|suffix| suffix.to_str())
                .is_some_and(|suffix| matches!(suffix, "yaml" | "yml"))
        {
            return Err(ComposeError::File);
        }
        let first = project.as_bytes().first();
        if project.len() > 255
            || !first.is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !project.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(ComposeError::Project);
        }
        Ok(Self {
            file,
            project,
            operation,
            remove_volumes: false,
        })
    }

    /// Authorize removing project volumes only after the exact project name was typed.
    pub fn down_with_volumes_for_selected_file(
        file: PathBuf,
        project: impl Into<String>,
        typed: &str,
    ) -> Result<Self, ComposeError> {
        let mut command = Self::for_selected_file(file, project, ComposeOperation::Down)?;
        if command.project != typed {
            return Err(ComposeError::Confirmation);
        }
        command.remove_volumes = true;
        Ok(command)
    }

    /// Launch the verified bundled process and retain its output until this stream drops.
    pub async fn start(self) -> Result<ComposeStream, ComposeError> {
        if geteuid().is_root() {
            return Err(ClientError::Root.into());
        }
        let project_file = tokio::fs::canonicalize(self.file)
            .await
            .map_err(ComposeError::Io)?;
        if !tokio::fs::metadata(&project_file)
            .await
            .map_err(ComposeError::Io)?
            .is_file()
        {
            return Err(ComposeError::File);
        }
        let project_directory = project_file.parent().ok_or(ComposeError::File)?;
        let binary = platform::bundled_compose()
            .await
            .map_err(ComposeError::Bundle)?;
        let bin_directory = binary
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or(ComposeError::File)?;
        let socket = docker_socket_path()?;
        // Apply the same owner and peer checks as typed Docker requests
        // before asking the bundled executable to use the endpoint.
        drop(
            platform::open_socket(socket.clone())
                .await
                .map_err(ComposeError::Io)?,
        );
        let socket = socket.to_str().ok_or(ComposeError::EndpointPath)?;
        let inherited_path = std::env::var_os("PATH");
        let path = std::env::join_paths(
            std::iter::once(bin_directory.to_path_buf())
                .chain(inherited_path.iter().flat_map(std::env::split_paths)),
        )
        .map_err(|_| ComposeError::EndpointPath)?;
        let mut command = Command::new(&binary);
        command
            .args(["--project-name", &self.project, "--project-directory"])
            .arg(project_directory)
            .arg("--file")
            .arg(&project_file)
            .args(["--ansi", "never"])
            .current_dir(project_directory)
            .env("DOCKER_HOST", format!("unix://{socket}"))
            .env("PATH", path)
            .env("OTEL_SDK_DISABLED", "true")
            .env_remove("DOCKER_CONTEXT")
            .env_remove("DOCKER_API_VERSION")
            .env_remove("DOCKER_CLI_PLUGIN_ORIGINAL_CLI_COMMAND")
            .env_remove("DOCKER_TLS")
            .env_remove("DOCKER_TLS_VERIFY")
            .env_remove("DOCKER_CERT_PATH")
            .env_remove("COMPOSE_FILE")
            .env_remove("COMPOSE_PROJECT_NAME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        match self.operation {
            ComposeOperation::Up => {
                command.args(["up", "--detach"]);
            }
            ComposeOperation::Down => {
                command.arg("down");
                if self.remove_volumes {
                    command.arg("--volumes");
                }
            }
            ComposeOperation::Restart => {
                command.arg("restart");
            }
        }
        let mut child = command.spawn().map_err(ComposeError::Io)?;
        let stdout = child
            .stdout
            .take()
            .map(BufReader::new)
            .ok_or(ComposeError::Output)?;
        let stderr = child
            .stderr
            .take()
            .map(BufReader::new)
            .ok_or(ComposeError::Output)?;
        Ok(ComposeStream {
            child,
            stdout: Some(stdout),
            stderr: Some(stderr),
            out_buffer: Vec::new(),
            err_buffer: Vec::new(),
            done: false,
        })
    }
}

/// Which pipe provided a piece of Compose output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeSource {
    /// Process standard output.
    Stdout,
    /// Process standard error.
    Stderr,
}

/// One line or bounded fragment emitted by Compose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeOutput {
    /// Original process pipe.
    pub source: ComposeSource,
    /// Text with invalid UTF-8 replaced.
    pub text: String,
}

/// Running Compose process and both output pipes.
pub struct ComposeStream {
    child: Child,
    stdout: Option<BufReader<ChildStdout>>,
    stderr: Option<BufReader<ChildStderr>>,
    out_buffer: Vec<u8>,
    err_buffer: Vec<u8>,
    done: bool,
}

impl ComposeStream {
    /// Read a bounded fragment. EOF indicates successful command completion.
    pub async fn next(&mut self) -> Result<Option<ComposeOutput>, ComposeError> {
        while self.stdout.is_some() || self.stderr.is_some() {
            tokio::select! {
                output = bounded_output(&mut self.stdout, &mut self.out_buffer),
                    if self.stdout.is_some() => {
                        if let Some(text) = output.map_err(ComposeError::Io)? {
                            return Ok(Some(ComposeOutput { source: ComposeSource::Stdout, text }));
                        }
                    }
                output = bounded_output(&mut self.stderr, &mut self.err_buffer),
                    if self.stderr.is_some() => {
                        if let Some(text) = output.map_err(ComposeError::Io)? {
                            return Ok(Some(ComposeOutput { source: ComposeSource::Stderr, text }));
                        }
                    }
            }
        }
        if !self.done {
            let status = self.child.wait().await.map_err(ComposeError::Io)?;
            self.done = true;
            if !status.success() {
                return Err(ComposeError::Exit(status));
            }
        }
        Ok(None)
    }
}

async fn bounded_output<R: AsyncBufRead + Unpin>(
    reader: &mut Option<R>,
    buffer: &mut Vec<u8>,
) -> io::Result<Option<String>> {
    let Some(pipe) = reader.as_mut() else {
        return Ok(None);
    };
    loop {
        let bytes = pipe.fill_buf().await?;
        if bytes.is_empty() {
            *reader = None;
            break;
        }
        let room = MAX_OUTPUT_LINE - buffer.len();
        let count = bytes
            .iter()
            .take(room)
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len().min(room), |position| position + 1);
        buffer.extend_from_slice(&bytes[..count]);
        pipe.consume(count);
        if buffer.len() == MAX_OUTPUT_LINE || buffer.ends_with(b"\n") {
            break;
        }
    }
    if buffer.is_empty() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(buffer).into_owned();
    buffer.clear();
    Ok(Some(text))
}

/// Errors reported while launching or reading the bundled Compose process.
#[derive(Debug, Error)]
pub enum ComposeError {
    /// The project file is not an absolute path to a YAML file.
    #[error("expected a local Compose YAML file")]
    File,
    /// Docker Compose project names are restricted to lowercase letters, numbers and separators.
    #[error("invalid Compose project name")]
    Project,
    /// The user-entered project name does not match.
    #[error("Compose project confirmation did not match")]
    Confirmation,
    /// A verified Compose executable is unavailable.
    #[error("bundled Compose executable is unavailable")]
    Bundle(#[source] io::Error),
    /// Client socket discovery or ownership validation failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The Docker endpoint or executable search path cannot be represented.
    #[error("could not configure the local Docker endpoint")]
    EndpointPath,
    /// The process failed to expose both output pipes.
    #[error("could not read Compose output")]
    Output,
    /// A filesystem, process or pipe operation failed.
    #[error("could not run Compose")]
    Io(#[source] io::Error),
    /// The executable returned failure after its output was delivered.
    #[error("Compose failed: {0}")]
    Exit(std::process::ExitStatus),
}
