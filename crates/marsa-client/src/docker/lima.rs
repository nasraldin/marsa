//! Lima instance settings are resolved from the selected Docker socket and
//! applied through the installed, unprivileged Lima CLI.

use std::{
    env, io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::Deserialize;
use thiserror::Error;
use tokio::{fs, io::AsyncReadExt, process::Command, sync::Mutex, time::timeout};

use crate::platform;

#[cfg(test)]
mod tests;

const OUTPUT_LIMIT: u64 = 1024 * 1024;
const CONFIG_LIMIT: usize = 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
// Lima's graceful shutdown may spend just over six minutes waiting for the
// guest and its host agent. Give the installed CLI time to finish that path.
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(400);
static MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

/// A local Lima VM identified by the exact Docker socket it forwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimaInstance {
    root: PathBuf,
    socket: PathBuf,
    name: String,
}

/// Effective Lima configuration and the current VM lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimaSettings {
    /// Configured virtual CPUs.
    pub cpus: u32,
    /// Configured guest RAM in bytes.
    pub memory_bytes: u64,
    /// Configured primary disk capacity in bytes, which Lima applies at startup.
    pub disk_bytes: u64,
    /// Whether Lima reports the instance as stopped.
    pub stopped: bool,
    /// Whether Lima reports the instance as running.
    pub running: bool,
    /// Whether guests can reach host loopback services, if this network supports the setting.
    pub host_loopback: Option<bool>,
}

/// Requested changes to a stopped Lima VM.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LimaSettingsUpdate {
    /// New virtual CPU count.
    pub cpus: Option<u32>,
    /// New guest RAM in bytes.
    pub memory_bytes: Option<u64>,
    /// New primary disk capacity in bytes, which may only increase.
    pub disk_bytes: Option<u64>,
    /// Whether to allow guest access to host loopback services.
    pub host_loopback: Option<bool>,
}

/// Errors when inspecting or changing owner-managed Lima settings.
#[derive(Debug, Error)]
pub enum LimaSettingsError {
    /// A compatible Lima CLI could not be found.
    #[error("Lima 2.3 or later in the 2.x series is required")]
    Unavailable,
    /// The configured directory or forward does not match the selected endpoint.
    #[error("the selected socket could not be matched to an owner-managed Lima Docker instance")]
    UnsafeInstance,
    /// Only a fully stopped VM may be edited.
    #[error("stop the Lima instance before editing its settings")]
    RequiresStopped,
    /// The requested capacity is below the configured disk size.
    #[error("Lima primary disks can only grow")]
    DiskShrink,
    /// A resource value falls outside the supported range.
    #[error("invalid Lima CPU, memory, or disk settings")]
    InvalidResources,
    /// The instance's driver or network does not support host loopback configuration.
    #[error("this Lima instance does not support changing host loopback access")]
    UnsupportedNetwork,
    /// The configuration changed since it was displayed.
    #[error("Lima settings changed since they were loaded")]
    Changed,
    /// Start and stop require Lima to report an unambiguous lifecycle state.
    #[error("Lima must report the instance as running or stopped")]
    LifecycleState,
    /// The requested VM state could not be reached with unprivileged commands.
    #[error("Lima could not complete the requested unprivileged lifecycle operation")]
    LifecycleFailed,
    /// Lima did not complete the operation or its returned settings differed.
    #[error("Lima could not complete the settings operation")]
    CommandFailed,
}

impl LimaInstance {
    /// Recognize the standard Lima Docker socket path, even after the VM stops.
    /// The claimed instance is verified against Lima before settings are read or written.
    pub fn from_socket(socket: &Path) -> Option<Self> {
        if !socket.is_absolute() {
            return None;
        }
        let directory = socket.parent()?.parent()?;
        let root = directory.parent()?;
        let configured_home = env::var_os("LIMA_HOME").map(PathBuf::from);
        if root.file_name().is_none_or(|name| name != ".lima")
            && configured_home.as_deref() != Some(root)
        {
            return None;
        }
        Some(Self {
            name: match_socket(socket, root)?.to_owned(),
            socket: socket.to_owned(),
            root: root.to_owned(),
        })
    }

    /// Name of the exact Lima instance represented by this endpoint.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Read settings for the exact configured Docker socket.
    pub async fn settings(&self) -> Result<LimaSettings, LimaSettingsError> {
        self.check_target().await?;
        let lima = LimaCommand::find().await?;
        self.read(&lima).await
    }

    /// Start this validated instance or shut it down gracefully, without host elevation.
    /// Returns the observed effective settings after the transition.
    pub async fn set_running(&self, running: bool) -> Result<LimaSettings, LimaSettingsError> {
        let _guard = MUTATION_LOCK.lock().await;
        let config = self.check_target().await?;
        let lima = LimaCommand::find().await?;
        let current = self.read(&lima).await?;
        self.transition(&lima, &config, current, running).await
    }

    /// Gracefully restart this exact running instance without changing its configuration.
    pub async fn restart(&self) -> Result<LimaSettings, LimaSettingsError> {
        let instance = self.clone();
        // A window can close between the two transitions. Keep the confirmed
        // restart running on the session runtime even if its caller goes away.
        tokio::spawn(async move { instance.restart_running().await })
            .await
            .map_err(|_| LimaSettingsError::LifecycleFailed)?
    }

    async fn restart_running(&self) -> Result<LimaSettings, LimaSettingsError> {
        let _guard = MUTATION_LOCK.lock().await;
        let config = self.check_target().await?;
        let lima = LimaCommand::find().await?;
        let current = self.read(&lima).await?;
        if !current.running {
            return Err(LimaSettingsError::LifecycleState);
        }
        let stopped = self.transition(&lima, &config, current, false).await?;
        self.transition(&lima, &config, stopped, true).await
    }

    async fn transition(
        &self,
        lima: &LimaCommand,
        config: &[u8],
        current: LimaSettings,
        running: bool,
    ) -> Result<LimaSettings, LimaSettingsError> {
        let Some(arguments) = lifecycle_arguments(current, running)? else {
            return Ok(current);
        };
        if self.check_target().await? != config {
            return Err(LimaSettingsError::Changed);
        }
        let mut command = platform::lima_lifecycle_command(&lima.0, &self.root.join(&self.name))
            .map_err(|_| LimaSettingsError::LifecycleFailed)?;
        lima.configure(&mut command, &self.root);
        command.args(arguments).args(["--", &self.name]);
        let completed = run_bounded(command, LIFECYCLE_TIMEOUT).await.is_ok();
        let actual = self.read(lima).await?;
        // Stop can finish successfully before Lima reconciles unrelated
        // networks. In that case report the verified stopped state.
        if (running && completed && actual.running) || (!running && actual.stopped) {
            return Ok(actual);
        }
        Err(LimaSettingsError::LifecycleFailed)
    }

    /// Edit the stopped instance and verify that Lima reports the requested values.
    pub async fn update(
        &self,
        expected: LimaSettings,
        update: LimaSettingsUpdate,
    ) -> Result<LimaSettings, LimaSettingsError> {
        let _guard = MUTATION_LOCK.lock().await;
        let config = self.check_target().await?;
        let lima = LimaCommand::find().await?;
        let current = self.read(&lima).await?;
        let requested = edit_values(current, expected, update)?;
        let changes = edit_arguments(current, requested);
        if changes.is_empty() {
            return Ok(current);
        }
        if self.check_target().await? != config {
            return Err(LimaSettingsError::Changed);
        }

        // Lima can emit a rejected config containing user data. Keep it in a
        // private temporary working directory, and never print process output.
        let workspace = tokio::task::spawn_blocking(tempfile::tempdir)
            .await
            .map_err(|_| LimaSettingsError::CommandFailed)?
            .map_err(|_| LimaSettingsError::CommandFailed)?;
        let mut command = lima.command(&self.root);
        command
            .current_dir(workspace.path())
            .args(["edit", "--start=false"])
            .args(changes)
            .args(["--", &self.name]);
        run(command).await?;

        let actual = self.read(&lima).await?;
        if actual != requested {
            return Err(LimaSettingsError::CommandFailed);
        }
        Ok(actual)
    }

    async fn read(&self, lima: &LimaCommand) -> Result<LimaSettings, LimaSettingsError> {
        let mut command = lima.command(&self.root);
        command.args(["list", "--format=json", "--all-fields", "--", &self.name]);
        decode_settings(&run(command).await?, &self.name, &self.socket, true)
    }

    async fn check_target(&self) -> Result<Vec<u8>, LimaSettingsError> {
        if nix::unistd::geteuid().is_root() {
            return Err(LimaSettingsError::UnsafeInstance);
        }
        let owner = tokio::task::spawn_blocking(|| {
            nix::unistd::User::from_uid(nix::unistd::geteuid())
                .ok()
                .flatten()
        })
        .await
        .ok()
        .flatten()
        .ok_or(LimaSettingsError::UnsafeInstance)?;
        let configured_home = env::var_os("LIMA_HOME").map(PathBuf::from);
        let owner_home = fs::canonicalize(&owner.dir)
            .await
            .map_err(|_| LimaSettingsError::UnsafeInstance)?
            .join(".lima");
        if self.root != owner_home && configured_home.as_deref() != Some(self.root.as_path()) {
            return Err(LimaSettingsError::UnsafeInstance);
        }
        platform::check_lima_target(&self.root, &self.name, &self.socket)
            .await
            .map_err(|_| LimaSettingsError::UnsafeInstance)?;
        platform::read_user_docker_file(
            &self.root.join(&self.name).join("lima.yaml"),
            CONFIG_LIMIT,
            false,
        )
        .await
        .map_err(|_| LimaSettingsError::UnsafeInstance)
    }
}

fn match_socket<'a>(socket: &'a Path, root: &Path) -> Option<&'a str> {
    let mut components = socket.strip_prefix(root).ok()?.components();
    let name = components.next()?.as_os_str().to_str()?;
    if !valid_name(name)
        || components.next()? != Component::Normal("sock".as_ref())
        || components.next()? != Component::Normal("docker.sock".as_ref())
        || components.next().is_some()
        || socket.as_os_str() != root.join(name).join("sock/docker.sock").as_os_str()
    {
        return None;
    }
    Some(name)
}

fn valid_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.len() <= 76
        && !lower.ends_with(".yaml")
        && !lower.ends_with(".yml")
        && name
            .split(['.', '-', '_'])
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn lifecycle_arguments(
    settings: LimaSettings,
    running: bool,
) -> Result<Option<&'static [&'static str]>, LimaSettingsError> {
    match (settings.running, settings.stopped, running) {
        (true, false, true) | (false, true, false) => Ok(None),
        (true, false, false) => Ok(Some(&["stop"])),
        (false, true, true) => Ok(Some(&["start", "--timeout=180s"])),
        _ => Err(LimaSettingsError::LifecycleState),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceRecord {
    name: String,
    status: String,
    dir: PathBuf,
    vm_type: String,
    cpus: u32,
    memory: u64,
    disk: u64,
    config: Config,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    port_forwards: Vec<PortForward>,
    #[serde(default)]
    networks: Vec<Network>,
    #[serde(default)]
    network_opts: NetworkOptions,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortForward {
    host_socket: Option<PathBuf>,
    guest_socket: Option<PathBuf>,
    #[serde(default)]
    ignore: bool,
}

#[derive(Deserialize)]
struct Network {
    lima: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkOptions {
    host_loopback: Option<bool>,
}

fn decode_settings(
    bytes: &[u8],
    name: &str,
    socket: &Path,
    supports_loopback: bool,
) -> Result<LimaSettings, LimaSettingsError> {
    let record: InstanceRecord =
        serde_json::from_slice(bytes).map_err(|_| LimaSettingsError::CommandFailed)?;
    let expected_directory = socket
        .parent()
        .and_then(Path::parent)
        .ok_or(LimaSettingsError::UnsafeInstance)?;
    if record.name != name
        || record.dir.as_os_str() != expected_directory.as_os_str()
        || record
            .config
            .port_forwards
            .iter()
            .filter(|rule| rule.host_socket.as_deref() == Some(socket))
            .count()
            != 1
        || !record.config.port_forwards.iter().any(|rule| {
            !rule.ignore
                && rule.host_socket.as_deref() == Some(socket)
                && rule.guest_socket.as_deref().is_some_and(|guest| {
                    guest.is_absolute()
                        && guest.file_name().is_some_and(|name| name == "docker.sock")
                })
        })
        || !matches!(record.vm_type.as_str(), "vz" | "qemu")
    {
        return Err(LimaSettingsError::UnsafeInstance);
    }
    if record.cpus == 0 || record.memory == 0 || record.disk == 0 {
        return Err(LimaSettingsError::InvalidResources);
    }
    // A named Lima attachment can represent a shared user-v2 network. Its
    // gateway policy is shared and cannot safely be edited by one VM.
    let host_loopback = (supports_loopback
        && record.vm_type == "vz"
        && record
            .config
            .networks
            .iter()
            .all(|network| network.lima.as_deref().is_none_or(str::is_empty)))
    .then(|| record.config.network_opts.host_loopback.unwrap_or(true));
    Ok(LimaSettings {
        cpus: record.cpus,
        memory_bytes: record.memory,
        disk_bytes: record.disk,
        stopped: record.status == "Stopped",
        running: record.status == "Running",
        host_loopback,
    })
}

fn edit_values(
    current: LimaSettings,
    expected: LimaSettings,
    update: LimaSettingsUpdate,
) -> Result<LimaSettings, LimaSettingsError> {
    if !current.stopped {
        return Err(LimaSettingsError::RequiresStopped);
    }
    if current != expected {
        return Err(LimaSettingsError::Changed);
    }
    if update.host_loopback.is_some() && current.host_loopback.is_none() {
        return Err(LimaSettingsError::UnsupportedNetwork);
    }
    let requested = LimaSettings {
        cpus: update.cpus.unwrap_or(current.cpus),
        memory_bytes: update.memory_bytes.unwrap_or(current.memory_bytes),
        disk_bytes: update.disk_bytes.unwrap_or(current.disk_bytes),
        stopped: true,
        running: false,
        host_loopback: update.host_loopback.or(current.host_loopback),
    };
    if requested.disk_bytes < current.disk_bytes {
        return Err(LimaSettingsError::DiskShrink);
    }
    // Lima's byte quantity parser uses a binary64 intermediate. Values below
    // 8 PiB round-trip exactly, including allocations expressed in whole bytes.
    if requested.cpus == 0
        || requested.cpus > i32::MAX as u32
        || requested.memory_bytes == 0
        || requested.memory_bytes >= 1 << 53
        || requested.disk_bytes == 0
        || requested.disk_bytes >= 1 << 53
    {
        return Err(LimaSettingsError::InvalidResources);
    }
    Ok(requested)
}

fn edit_arguments(current: LimaSettings, requested: LimaSettings) -> Vec<String> {
    let mut args = Vec::new();
    if requested.cpus != current.cpus {
        args.extend(["--cpus".into(), requested.cpus.to_string()]);
    }
    if requested.memory_bytes != current.memory_bytes {
        args.extend([
            "--set".into(),
            format!(r#".memory = "{}B""#, requested.memory_bytes),
        ]);
    }
    if requested.disk_bytes != current.disk_bytes {
        args.extend([
            "--set".into(),
            format!(r#".disk = "{}B""#, requested.disk_bytes),
        ]);
    }
    if let Some(loopback) = requested
        .host_loopback
        .filter(|_| requested.host_loopback != current.host_loopback)
    {
        args.extend([
            "--set".into(),
            format!(".networkOpts.hostLoopback = {loopback}"),
        ]);
    }
    args
}

struct LimaCommand(PathBuf);

impl LimaCommand {
    async fn find() -> Result<Self, LimaSettingsError> {
        let mut candidates = vec![
            PathBuf::from("/opt/homebrew/bin/limactl"),
            PathBuf::from("/usr/local/bin/limactl"),
        ];
        if let Some(path) = env::var_os("PATH") {
            candidates.extend(
                env::split_paths(&path)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join("limactl")),
            );
        }
        let mut command = None;
        for candidate in candidates {
            if let Ok(path) = platform::trusted_lima_binary(&candidate).await {
                command = Some(Self(path));
                break;
            }
        }
        let lima = command.ok_or(LimaSettingsError::Unavailable)?;
        let mut command = Command::new(&lima.0);
        command.arg("--version");
        let version = run(command)
            .await
            .map_err(|_| LimaSettingsError::Unavailable)?;
        let version = std::str::from_utf8(&version)
            .ok()
            .and_then(|text| text.trim().strip_prefix("limactl version "))
            .and_then(|text| text.strip_prefix("2."))
            .and_then(|text| text.split_once('.'))
            .and_then(|(minor, _)| minor.parse::<u32>().ok());
        if version.is_none_or(|minor| minor < 3) {
            return Err(LimaSettingsError::Unavailable);
        }
        Ok(lima)
    }

    fn command(&self, root: &Path) -> Command {
        let mut command = Command::new(&self.0);
        self.configure(&mut command, root);
        command
    }

    fn configure(&self, command: &mut Command, root: &Path) {
        command.env("LIMA_HOME", root).arg("--tty=false");
    }
}

async fn run(command: Command) -> Result<Vec<u8>, LimaSettingsError> {
    run_bounded(command, COMMAND_TIMEOUT).await
}

async fn run_bounded(
    mut command: Command,
    deadline: Duration,
) -> Result<Vec<u8>, LimaSettingsError> {
    timeout(deadline, async move {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
        let mut output = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut output)
            .await?;
        if output.len() as u64 > OUTPUT_LIMIT || !child.wait().await?.success() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok::<_, io::Error>(output)
    })
    .await
    .map_err(|_| LimaSettingsError::CommandFailed)?
    .map_err(|_| LimaSettingsError::CommandFailed)
}
