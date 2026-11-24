//! Implement the CLI's engine commands over the local typed daemon socket.

use std::{
    fmt::Write as _,
    io::{self, Write},
    time::Duration,
};

use clap::ArgMatches;
use marsa_client::{
    ClientError, DaemonEndpoint, MarsaClient,
    api::{
        EngineActionRequest, EngineHealthState, EngineState, EngineStatus, EngineStatusRequest,
        LogEntry, LogsRequest, VmStatsRequest, VmStatsSample, daemon_event::Body,
    },
    docker_socket_path,
    l10n::{MessageKey, keys},
};
use serde_json::{Value, json};

use crate::{Failure, Outcome, command};

const TAIL_LINES: u32 = 100;
const STATS_WAIT: Duration = Duration::from_millis(1500);

pub(super) async fn run(command: &str, args: &ArgMatches, machine: bool) -> Option<Outcome> {
    let command = match command {
        "env" => return Some(environment().unwrap_or_else(Outcome::Failure)),
        "status" => Command::Status,
        "version" => Command::Version,
        "start" => Command::Lifecycle(Action::Start),
        "stop" => Command::Lifecycle(Action::Stop),
        "restart" => Command::Lifecycle(Action::Restart),
        "logs" => Command::Logs {
            follow: args.get_flag("follow"),
        },
        _ => return None,
    };
    Some(command.run(machine).await.unwrap_or_else(Outcome::Failure))
}

enum Command {
    Status,
    Version,
    Lifecycle(Action),
    Logs { follow: bool },
}

impl Command {
    async fn run(self, machine: bool) -> Result<Outcome, Failure> {
        let client = connect().await?;
        match self {
            Self::Status => status(&client).await,
            Self::Version => Ok(version(&client)),
            Self::Lifecycle(action) => lifecycle(&client, action, machine).await,
            Self::Logs { follow } => logs(&client, follow, machine).await,
        }
    }
}

pub(super) async fn connect() -> Result<MarsaClient, Failure> {
    DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)
}

pub(super) fn connection_failure(error: ClientError) -> Failure {
    match error {
        ClientError::Home => Failure::Home,
        ClientError::Root => Failure::Root,
        ClientError::IncompatibleVersion { .. } | ClientError::MissingVersionService => {
            Failure::Incompatible
        }
        _ => Failure::Connection,
    }
}

fn environment() -> Result<Outcome, Failure> {
    let path = docker_socket_path().map_err(connection_failure)?;
    let path = path.to_str().ok_or(Failure::Home)?;
    let host = format!("unix://{path}");
    // POSIX single-quote escaping keeps even shell syntax in the home path literal.
    let quoted = host.replace('\'', "'\"'\"'");
    Ok(Outcome::Data {
        document: json!({ "schema_version": 1, "kind": "env", "docker_host": host }),
        text: format!("export DOCKER_HOST='{quoted}'\n"),
    })
}

async fn status(client: &MarsaClient) -> Result<Outcome, Failure> {
    let engine = client
        .engine()
        .status(EngineStatusRequest {})
        .await
        .map_err(|_| Failure::Operation)?
        .into_inner();
    let resources = match engine.state() {
        EngineState::Ready | EngineState::Idle | EngineState::Paused => {
            tokio::time::timeout(STATS_WAIT, sample(client))
                .await
                .ok()
                .flatten()
        }
        _ => None,
    };
    let mut text = engine_text(&engine);
    text.push_str(&versions_text(client));
    if let Some(stats) = &resources {
        let _ = writeln!(
            text,
            "{}\n{}\n{}",
            technical_row(keys::CLI_ENGINE_CPU_TIME, &stats.cpu_busy_nanos.to_string()),
            technical_row(
                keys::CLI_ENGINE_MEMORY,
                &format!("{}/{}", stats.memory_used_bytes, stats.memory_total_bytes)
            ),
            technical_row(
                keys::CLI_ENGINE_DISK,
                &format!("{}/{}", stats.disk_used_bytes, stats.disk_total_bytes)
            ),
        );
    }
    Ok(Outcome::Data {
        document: json!({
            "schema_version": 1,
            "kind": "status",
            "engine": engine_data(&engine),
            "versions": versions_data(client),
            "resources": resources.as_ref().map(resources_data),
        }),
        text,
    })
}

async fn sample(client: &MarsaClient) -> Option<VmStatsSample> {
    let mut stream = client
        .stats()
        .vm_stats(VmStatsRequest {})
        .await
        .ok()?
        .into_inner();
    stream.message().await.ok().flatten()
}

fn resources_data(sample: &VmStatsSample) -> Value {
    json!({
        "observed_at_unix_millis": sample.observed_at_unix_millis,
        "cpu_busy_nanos": sample.cpu_busy_nanos,
        "memory_used_bytes": sample.memory_used_bytes,
        "memory_total_bytes": sample.memory_total_bytes,
        "disk_used_bytes": sample.disk_used_bytes,
        "disk_total_bytes": sample.disk_total_bytes,
        "network_rx_bytes": sample.network_rx_bytes,
        "network_tx_bytes": sample.network_tx_bytes,
    })
}

fn versions_data(client: &MarsaClient) -> Value {
    let version = client.version();
    let components: Vec<_> = version
        .components
        .iter()
        .map(|component| json!({ "name": component.name, "version": component.version }))
        .collect();
    json!({
        "cli": env!("CARGO_PKG_VERSION"),
        "daemon": version.daemon_version,
        "api": { "major": version.api_major, "minor": client.api_minor() },
        "components": components,
    })
}

fn versions_text(client: &MarsaClient) -> String {
    let version = client.version();
    let mut text = format!(
        "{}\n{}\n{}\n",
        technical_row(keys::CLI_ENGINE_CLI_VERSION, env!("CARGO_PKG_VERSION")),
        technical_row(keys::CLI_ENGINE_DAEMON_VERSION, &version.daemon_version),
        technical_row(
            keys::CLI_ENGINE_API_VERSION,
            &format!("{}.{}", version.api_major, client.api_minor())
        ),
    );
    for component in &version.components {
        let _ = writeln!(
            text,
            "{}",
            command::technical(&format!("{} {}", component.name, component.version))
        );
    }
    text
}

fn version(client: &MarsaClient) -> Outcome {
    Outcome::Data {
        document: json!({ "schema_version": 1, "kind": "version", "versions": versions_data(client) }),
        text: versions_text(client),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    fn progress(self) -> &'static str {
        match self {
            Self::Start => command::label(keys::CLI_ENGINE_STARTING_ENGINE),
            Self::Stop => command::label(keys::CLI_ENGINE_STOPPING_ENGINE),
            Self::Restart => command::label(keys::CLI_ENGINE_RESTARTING_ENGINE),
        }
    }
}

async fn lifecycle(
    client: &MarsaClient,
    action: Action,
    machine: bool,
) -> Result<Outcome, Failure> {
    let request = EngineActionRequest {
        request_id: request_id()?,
    };
    let mut engine_client = client.engine();
    let operation = async move {
        match action {
            Action::Start => engine_client.start(request).await,
            Action::Stop => engine_client.stop(request).await,
            Action::Restart => engine_client.restart(request).await,
        }
    };
    let result = if machine {
        operation.await
    } else {
        let _ = writeln!(io::stderr().lock(), "{}", action.progress());
        let mut events = client.subscribe_events();
        let mut state = None;
        let mut watch = true;
        tokio::pin!(operation);
        loop {
            tokio::select! {
                biased;
                result = &mut operation => break result,
                event = events.next(), if watch => {
                    match event {
                        Ok(event) => {
                            if let Some(Body::Engine(engine)) = event.body {
                                let next = engine.state();
                                if state != Some(next) {
                                    state = Some(next);
                                    let _ = writeln!(io::stderr().lock(), "{}", state_name(next));
                                }
                            }
                        }
                        Err(_) => watch = false,
                    }
                }
            }
        }
    };
    let engine = result.map_err(|_| Failure::Operation)?.into_inner();
    let expected = match action {
        Action::Stop => engine.state() == EngineState::Stopped,
        Action::Start | Action::Restart => {
            matches!(engine.state(), EngineState::Ready | EngineState::Idle)
        }
    };
    if !expected {
        return Err(Failure::Operation);
    }
    Ok(Outcome::Data {
        document: json!({
            "schema_version": 1,
            "kind": "lifecycle",
            "action": action.name(),
            "engine": engine_data(&engine),
        }),
        text: engine_text(&engine),
    })
}

pub(super) fn request_id() -> Result<String, Failure> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Failure::Operation)?;
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let mut uuid = String::with_capacity(36);
    for (index, byte) in bytes.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            uuid.push('-');
        }
        let _ = write!(uuid, "{byte:02x}");
    }
    Ok(uuid)
}

fn engine_data(engine: &EngineStatus) -> Value {
    let state = engine
        .state()
        .as_str_name()
        .trim_start_matches("ENGINE_STATE_")
        .to_ascii_lowercase();
    let health = engine
        .guest_health()
        .as_str_name()
        .trim_start_matches("ENGINE_HEALTH_STATE_")
        .to_ascii_lowercase();
    json!({ "state": state, "backend": engine.backend, "guest_health": health, "boot_stage": engine.boot_stage })
}

fn engine_text(engine: &EngineStatus) -> String {
    format!(
        "{}\n{}\n{}\n",
        command::message(
            keys::CLI_ENGINE_STATE,
            &[("state", state_name(engine.state()))]
        ),
        technical_row(keys::CLI_ENGINE_BACKEND, &engine.backend),
        command::message(
            keys::CLI_ENGINE_GUEST_HEALTH,
            &[("state", health_name(engine.guest_health()))]
        ),
    )
}

fn technical_row(key: MessageKey, value: &str) -> String {
    command::message(key, &[("value", &command::technical(value))])
}

fn state_name(state: EngineState) -> &'static str {
    match state {
        EngineState::Stopped => command::label(keys::CLI_ENGINE_STATE_STOPPED),
        EngineState::Starting => command::label(keys::CLI_ENGINE_STATE_STARTING),
        EngineState::Booting => command::label(keys::CLI_ENGINE_STATE_BOOTING),
        EngineState::Ready => command::label(keys::CLI_ENGINE_STATE_READY),
        EngineState::Idle => command::label(keys::CLI_ENGINE_STATE_IDLE),
        EngineState::Paused => command::label(keys::CLI_ENGINE_STATE_PAUSED),
        EngineState::Stopping => command::label(keys::CLI_ENGINE_STATE_STOPPING),
        EngineState::Failed => command::label(keys::CLI_ENGINE_STATE_FAILED),
        EngineState::Unspecified => command::label(keys::CLI_ENGINE_STATE_UNKNOWN),
    }
}

fn health_name(state: EngineHealthState) -> &'static str {
    match state {
        EngineHealthState::Starting => command::label(keys::CLI_ENGINE_STATE_STARTING),
        EngineHealthState::Ready => command::label(keys::CLI_ENGINE_STATE_READY),
        EngineHealthState::Unhealthy => command::label(keys::CLI_ENGINE_HEALTH_UNHEALTHY),
        EngineHealthState::Restarting => command::label(keys::CLI_ENGINE_HEALTH_RESTARTING),
        EngineHealthState::Unspecified => command::label(keys::CLI_ENGINE_STATE_UNKNOWN),
    }
}

async fn logs(client: &MarsaClient, follow: bool, machine: bool) -> Result<Outcome, Failure> {
    let mut stream = client
        .diagnostics()
        .logs(LogsRequest {
            follow,
            tail_lines: TAIL_LINES,
        })
        .await
        .map_err(|_| Failure::Operation)?
        .into_inner();
    let mut text = String::new();
    let mut entries = Vec::new();
    while let Some(entry) = stream.message().await.map_err(|_| Failure::Operation)? {
        let line = log_text(&entry);
        if follow {
            let result = if machine {
                let record =
                    json!({ "schema_version": 1, "kind": "log", "entry": log_data(&entry) });
                writeln!(io::stdout().lock(), "{record}")
            } else {
                write!(io::stdout().lock(), "{line}")
            };
            match result {
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                    return Ok(Outcome::Streamed);
                }
                Err(_) => return Err(Failure::Operation),
                Ok(()) => {}
            }
        } else {
            if entries.len() >= TAIL_LINES as usize {
                return Err(Failure::Operation);
            }
            entries.push(log_data(&entry));
            text.push_str(&line);
        }
    }
    if follow {
        return Ok(Outcome::Streamed);
    }
    Ok(Outcome::Data {
        document: json!({ "schema_version": 1, "kind": "logs", "entries": entries }),
        text,
    })
}

fn log_text(entry: &LogEntry) -> String {
    let row = format!(
        "{} {} {} {}",
        entry.at_unix_millis, entry.component, entry.level, entry.message
    );
    format!("{}\n", command::technical(&row))
}

fn log_data(entry: &LogEntry) -> Value {
    json!({
        "at_unix_millis": entry.at_unix_millis,
        "component": entry.component,
        "level": entry.level,
        "message": entry.message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_the_engine_row_without_modifying_protocol_values() {
        let engine = EngineStatus {
            state: EngineState::Ready.into(),
            guest_health: EngineHealthState::Ready.into(),
            backend: "fixture\u{202e}".into(),
            ..Default::default()
        };
        let english = engine_text(&engine);
        assert!(english.starts_with("Engine state Ready\n"));
        assert!(english.contains("VM backend fixture\\u{202e}"));
        assert!(!english.contains('\u{2066}'));
        assert_eq!(engine_data(&engine)["backend"], "fixture\u{202e}");
    }
}
