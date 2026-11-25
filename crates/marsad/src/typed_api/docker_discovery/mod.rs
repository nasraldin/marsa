//! Discover Marsa's endpoint and create a Docker context on explicit local request.

mod keychain;
mod store;
#[cfg(target_os = "macos")]
pub use store::{ContextRemoval, ContextRemovalError, prepare_context_removal};

use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use marsa_core::paths::MarsaPaths;
use marsa_proto::host::v1::{
    DockerEndpoint, EnableRegistryKeychainRequest, EnableRegistryKeychainResponse,
    EnsureDockerContextRequest, EnsureDockerContextResponse, GetDockerEndpointRequest,
    docker_discovery_server::DockerDiscovery,
};
use tokio::{process::Command, sync::Mutex, time::timeout};
use tonic::{Request, Response, Status};

use super::TypedApi;

pub(super) type VerifyKeychain = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;

const NAME: &str = "marsa";
const DESCRIPTION: &str = "Marsa";
const CLI_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct Registration {
    config: PathBuf,
    cli: Box<dyn Fn() -> io::Result<PathBuf> + Send + Sync>,
    write: Mutex<()>,
}

impl TypedApi {
    /// Enable explicit context registration using the packaged CLI and the host's Docker config directory.
    ///
    /// The provider must resolve a verified executable. No request supplies commands or paths.
    pub fn with_docker_contexts(
        mut self,
        config: PathBuf,
        cli: impl Fn() -> io::Result<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        self.docker_contexts = Some(Arc::new(Registration {
            config,
            cli: Box::new(cli),
            write: Mutex::new(()),
        }));
        self
    }
}

#[tonic::async_trait]
impl DockerDiscovery for TypedApi {
    async fn get_endpoint(
        &self,
        request: Request<GetDockerEndpointRequest>,
    ) -> Result<Response<DockerEndpoint>, Status> {
        self.authorize(request)?;
        Ok(Response::new(endpoint(&self.inner.paths)?))
    }

    async fn ensure_context(
        &self,
        request: Request<EnsureDockerContextRequest>,
    ) -> Result<Response<EnsureDockerContextResponse>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let response = self
            .inner
            .docker_context_requests
            .run(&request.request_id, (), || async move {
                let endpoint = endpoint(&api.inner.paths)?;
                let registration = api.docker_contexts.ok_or_else(|| {
                    Status::failed_precondition("the bundled Docker CLI is unavailable")
                })?;
                let created = registration
                    .ensure(&endpoint.docker_host, api.inner.uid)
                    .await?;
                Ok(EnsureDockerContextResponse {
                    endpoint: Some(endpoint),
                    created,
                })
            })
            .await?;
        Ok(Response::new(response))
    }

    async fn enable_registry_keychain(
        &self,
        request: Request<EnableRegistryKeychainRequest>,
    ) -> Result<Response<EnableRegistryKeychainResponse>, Status> {
        let request = self.authorize(request)?;
        let api = self.clone();
        let configured = self
            .inner
            .docker_keychain_requests
            .run(&request.request_id, (), || async move {
                let registration = api.docker_contexts.ok_or_else(unavailable_keychain)?;
                let verify = api.verify_keychain.ok_or_else(unavailable_keychain)?;
                let _write = registration.write.lock().await;
                let config = registration.config.clone();
                let uid = api.inner.uid;
                tokio::task::spawn_blocking(move || {
                    verify().map_err(|_| unavailable_keychain())?;
                    keychain::enable(&config, uid).map_err(keychain::status)
                })
                .await
                .map_err(|_| Status::internal("the Docker configuration worker stopped"))?
            })
            .await?;
        Ok(Response::new(EnableRegistryKeychainResponse { configured }))
    }
}

impl TypedApi {
    /// Enable the registry Keychain opt-in with a packaged credential helper verifier.
    pub fn with_registry_keychain(
        mut self,
        verify: impl Fn() -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.verify_keychain = Some(Arc::new(verify));
        self
    }
}

fn unavailable_keychain() -> Status {
    Status::failed_precondition("the bundled macOS credential helper is unavailable")
}

impl Registration {
    async fn ensure(self: Arc<Self>, host: &str, uid: u32) -> Result<bool, Status> {
        let _write = self.write.lock().await;
        let registration = self.clone();
        let (cli, stage) = tokio::task::spawn_blocking(move || {
            let cli = (registration.cli)().map_err(|_| {
                Status::failed_precondition("the bundled Docker CLI is unavailable")
            })?;
            if !cli.is_absolute() {
                return Err(Status::failed_precondition(
                    "the bundled Docker CLI is unavailable",
                ));
            }
            let stage = tempfile::tempdir()
                .map_err(|_| Status::internal("could not stage the Docker context"))?;
            Ok::<_, Status>((cli, stage))
        })
        .await
        .map_err(|_| Status::internal("the Docker context worker stopped"))??;
        create_context(&cli, stage.path(), host).await?;
        let config = self.config.clone();
        let host = host.to_owned();
        tokio::task::spawn_blocking(move || store::install(stage.path(), &config, uid, &host))
            .await
            .map_err(|_| Status::internal("the Docker context worker stopped"))?
            .map_err(store_status)
    }
}

fn endpoint(paths: &MarsaPaths) -> Result<DockerEndpoint, Status> {
    let socket = paths.docker_socket();
    let socket_path = socket
        .to_str()
        .filter(|path| !path.chars().any(char::is_control))
        .ok_or_else(|| {
            Status::failed_precondition("the Docker socket path is not representable")
        })?;
    Ok(DockerEndpoint {
        socket_path: socket_path.to_owned(),
        docker_host: format!("unix://{socket_path}"),
        context_name: NAME.into(),
    })
}

async fn create_context(cli: &Path, config: &Path, host: &str) -> Result<(), Status> {
    // pflag's StringToString parser reads a lone assignment directly. For a
    // socket name that contains '=', pass the assignment as a single CSV field.
    let mut argument = format!("host={host}");
    if host.contains('=') {
        argument = format!("\"{}\"", argument.replace('"', "\"\""));
    }
    let mut command = Command::new(cli);
    command
        .env_clear()
        .env("HOME", config)
        .current_dir(config)
        .arg("--config")
        .arg(config)
        .args([
            "context",
            "create",
            NAME,
            "--description",
            DESCRIPTION,
            "--docker",
        ])
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = timeout(CLI_TIMEOUT, command.status())
        .await
        .map_err(|_| Status::deadline_exceeded("Docker context creation timed out"))?
        .map_err(|_| Status::internal("could not run the bundled Docker CLI"))?;
    if !status.success() {
        return Err(Status::failed_precondition(
            "Docker could not create the Marsa context",
        ));
    }
    Ok(())
}

fn store_status(error: store::Error) -> Status {
    match error {
        store::Error::Conflict => Status::already_exists(error.to_string()),
        store::Error::Unsafe | store::Error::InvalidDirectory => {
            Status::failed_precondition(error.to_string())
        }
        store::Error::Io(error) => {
            tracing::warn!(kind = ?error.kind(), "could not install Docker context");
            Status::internal("could not install the Docker context")
        }
        store::Error::Stage => Status::internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
