//! Scoped Compose processes using a deliberately selected local project file.

use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::Stdio,
};

use nix::unistd::geteuid;
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout, Command},
};

use crate::{
    ClientError,
    docker::{DockerClient, ExternalEndpoint},
    docker_socket_path, platform,
};

mod protected;
mod relay;

const MAX_OUTPUT_LINE: usize = 64 * 1024;
const PROJECT_FILE: &str = "compose.yaml";

/// Starter YAML used when the desktop owner has not set a Compose template.
pub const DEFAULT_TEMPLATE: &str = "services:\n  app:\n    image: nginx:alpine\n";

/// Check the project name before offering a deployment.
pub fn valid_project_name(project: &str) -> bool {
    (1..=255).contains(&project.len())
        && project
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && project.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

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
    keychain: bool,
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
        if !valid_project_name(&project) {
            return Err(ComposeError::Project);
        }
        Ok(Self {
            file,
            project,
            operation,
            remove_volumes: false,
            keychain: false,
        })
    }

    /// Publish the reviewed YAML in an owner-selected project directory and prepare its deployment.
    ///
    /// An existing `compose.yaml`, including a symlink, is preserved. The new file
    /// remains available to edit and deploy again if Compose cannot start the stack.
    pub async fn for_new_file(
        directory: &Path,
        project: impl Into<String>,
        yaml: impl Into<String>,
    ) -> Result<Self, ComposeError> {
        if geteuid().is_root() {
            return Err(ClientError::Root.into());
        }
        let mut command =
            Self::for_selected_file(directory.join(PROJECT_FILE), project, ComposeOperation::Up)?;
        let directory = directory.to_path_buf();
        let yaml = yaml.into();
        if yaml.trim().is_empty() || yaml.contains('\0') {
            return Err(ComposeError::InvalidTemplate);
        }

        command.file = tokio::task::spawn_blocking(move || publish_project_file(&directory, &yaml))
            .await
            .map_err(|error| ComposeError::Io(io::Error::other(error)))??;
        Ok(command)
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

    /// Opt an approved deployment into the dedicated Keychain secret route.
    pub fn with_keychain(mut self) -> Result<Self, ComposeError> {
        if self.operation != ComposeOperation::Up {
            return Err(ComposeError::ProtectedModel);
        }
        self.keychain = true;
        Ok(self)
    }

    /// Launch the verified bundled process and retain its output until this stream drops.
    pub async fn start(self) -> Result<ComposeStream, ComposeError> {
        self.start_on(None).await
    }

    /// Run the selected project against the supplied pinned external endpoint, or Marsa.
    pub async fn start_on(
        self,
        endpoint: Option<ExternalEndpoint>,
    ) -> Result<ComposeStream, ComposeError> {
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
        let keychain_engine = self
            .keychain
            .then(|| {
                endpoint
                    .as_ref()
                    .map(ExternalEndpoint::management_client)
                    .map_or_else(DockerClient::discover, Ok)
                    .map_err(|_| ComposeError::ProtectedModel)
            })
            .transpose()?;
        let relay = endpoint
            .map(relay::Relay::start)
            .transpose()
            .map_err(ComposeError::Io)?;
        let socket = if let Some(relay) = &relay {
            relay.socket().to_path_buf()
        } else {
            let socket = docker_socket_path()?;
            drop(
                platform::open_socket(socket.clone())
                    .await
                    .map_err(ComposeError::Io)?,
            );
            socket
        };
        let socket = socket.to_str().ok_or(ComposeError::EndpointPath)?;
        if let Some(engine) = keychain_engine {
            return protected::start(
                &binary,
                &project_file,
                &self.project,
                socket,
                relay,
                &engine,
            )
            .await;
        }
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
            _relay: relay,
            _private_directory: None,
            stdout: Some(stdout),
            stderr: Some(stderr),
            out_buffer: Vec::new(),
            err_buffer: Vec::new(),
            done: false,
        })
    }
}

fn publish_project_file(directory: &Path, yaml: &str) -> Result<PathBuf, ComposeError> {
    let directory = directory.canonicalize().map_err(ComposeError::Io)?;
    let destination = directory.join(PROJECT_FILE);
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(ComposeError::Io)?;
    temporary
        .write_all(yaml.as_bytes())
        .map_err(ComposeError::Io)?;
    temporary.as_file().sync_all().map_err(ComposeError::Io)?;
    temporary.persist_noclobber(&destination).map_err(|error| {
        if error.error.kind() == io::ErrorKind::AlreadyExists {
            ComposeError::ExistingProjectFile
        } else {
            ComposeError::Io(error.error)
        }
    })?;
    Ok(destination)
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
    _relay: Option<relay::Relay>,
    _private_directory: Option<tempfile::TempDir>,
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
    /// The normalized project cannot be deployed with saved Keychain credentials.
    #[error("this Compose model does not support protected deploy secrets")]
    ProtectedModel,
    /// The requested secret could not be resolved from the signed Keychain provider.
    #[error(transparent)]
    DeploySecret(#[from] crate::docker::DeploySecretError),
    /// The project file is not an absolute path to a YAML file.
    #[error("expected a local Compose YAML file")]
    File,
    /// The requested destination already has a Compose file or directory.
    #[error("compose.yaml is already present in the project directory")]
    ExistingProjectFile,
    /// The new Compose content is blank or contains a NUL byte.
    #[error("invalid Compose template")]
    InvalidTemplate,
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

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "services:\n  web:\n    image: nginx:alpine\n";

    #[tokio::test]
    async fn create_prepares_exact_bytes_and_keeps_original_project_file() {
        let directory = tempfile::tempdir().unwrap();
        let command = ComposeCommand::for_new_file(directory.path(), "hello-stack", YAML)
            .await
            .unwrap();
        let path = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("compose.yaml");
        assert_eq!(command.file, path);
        assert_eq!(command.project, "hello-stack");
        assert_eq!(command.operation, ComposeOperation::Up);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), YAML);

        let error = ComposeCommand::for_new_file(directory.path(), "hello-stack", "services: {}\n")
            .await
            .unwrap_err();
        assert!(matches!(error, ComposeError::ExistingProjectFile));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), YAML);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn create_never_replaces_a_symlink_or_writes_for_an_invalid_project() {
        let directory = tempfile::tempdir().unwrap();
        let invalid = ComposeCommand::for_new_file(directory.path(), "bad/name", YAML)
            .await
            .unwrap_err();
        assert!(matches!(invalid, ComposeError::Project));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

        let target = directory.path().join("original");
        std::fs::write(&target, "original").unwrap();
        let path = directory.path().join("compose.yaml");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let existing = ComposeCommand::for_new_file(directory.path(), "hello-stack", YAML)
            .await
            .unwrap_err();
        assert!(matches!(existing, ComposeError::ExistingProjectFile));
        assert!(path.is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
    }
}
