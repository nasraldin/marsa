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
};
use serde_json::{Value, json};

use crate::{Failure, Outcome, command::Language};

const TAIL_LINES: u32 = 100;
const STATS_WAIT: Duration = Duration::from_millis(1500);

pub(super) async fn run(
    command: &str,
    args: &ArgMatches,
    language: Language,
    machine: bool,
) -> Option<Outcome> {
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
    Some(
        command
            .run(language, machine)
            .await
            .unwrap_or_else(Outcome::Failure),
    )
}

enum Command {
    Status,
    Version,
    Lifecycle(Action),
    Logs { follow: bool },
}

impl Command {
    async fn run(self, language: Language, machine: bool) -> Result<Outcome, Failure> {
        let client = DaemonEndpoint::discover()
            .map_err(connection_failure)?
            .connect()
            .await
            .map_err(connection_failure)?;
        match self {
            Self::Status => status(&client, language).await,
            Self::Version => Ok(version(&client, language)),
            Self::Lifecycle(action) => lifecycle(&client, action, language, machine).await,
            Self::Logs { follow } => logs(&client, follow, machine).await,
        }
    }
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

async fn status(client: &MarsaClient, language: Language) -> Result<Outcome, Failure> {
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
    let mut text = engine_text(language, &engine);
    text.push_str(&versions_text(client, language));
    if let Some(stats) = &resources {
        let cpu = language.text(
            "Cumulative CPU time (ns)",
            "وقت المعالج التراكمي (نانوثانية)",
        );
        let memory = language.text("Memory (bytes)", "الذاكرة (بايت)");
        let disk = language.text("Guest disk (bytes)", "قرص النظام الضيف (بايت)");
        let _ = writeln!(text, "{cpu} {}", stats.cpu_busy_nanos);
        let _ = writeln!(
            text,
            "{memory} {}/{}",
            stats.memory_used_bytes, stats.memory_total_bytes
        );
        let _ = writeln!(
            text,
            "{disk} {}/{}",
            stats.disk_used_bytes, stats.disk_total_bytes
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

fn versions_text(client: &MarsaClient, language: Language) -> String {
    let version = client.version();
    let cli = language.text("Marsa CLI", "سطر أوامر مرسى");
    let daemon = language.text("Marsa daemon", "خادم مرسى");
    let api = language.text("Daemon API", "واجهة الخادم");
    let mut text = format!(
        "{cli} {}\n{daemon} {}\n{api} {}.{}\n",
        env!("CARGO_PKG_VERSION"),
        version.daemon_version,
        version.api_major,
        client.api_minor(),
    );
    for component in &version.components {
        let _ = writeln!(text, "{} {}", component.name, component.version);
    }
    text
}

fn version(client: &MarsaClient, language: Language) -> Outcome {
    Outcome::Data {
        document: json!({ "schema_version": 1, "kind": "version", "versions": versions_data(client) }),
        text: versions_text(client, language),
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

    fn progress(self, language: Language) -> &'static str {
        match self {
            Self::Start => language.text("Starting the engine…", "جارٍ تشغيل المحرك…"),
            Self::Stop => language.text("Stopping the engine…", "جارٍ إيقاف المحرك…"),
            Self::Restart => language.text("Restarting the engine…", "جارٍ إعادة تشغيل المحرك…"),
        }
    }
}

async fn lifecycle(
    client: &MarsaClient,
    action: Action,
    language: Language,
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
        let _ = writeln!(io::stderr().lock(), "{}", action.progress(language));
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
                                    let _ = writeln!(io::stderr().lock(), "{}", state_name(language, next));
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
        text: engine_text(language, &engine),
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

fn engine_text(language: Language, engine: &EngineStatus) -> String {
    let state = language.text("Engine state", "حالة المحرك");
    let backend = language.text("VM backend", "واجهة تشغيل الآلة الافتراضية");
    let health = language.text("Guest engine health", "سلامة محرك النظام الضيف");
    format!(
        "{state} {}\n{backend} {}\n{health} {}\n",
        state_name(language, engine.state()),
        engine.backend,
        health_name(language, engine.guest_health()),
    )
}

fn state_name(language: Language, state: EngineState) -> &'static str {
    match state {
        EngineState::Stopped => language.text("Stopped", "متوقف"),
        EngineState::Starting => language.text("Starting", "جارٍ التشغيل"),
        EngineState::Booting => {
            language.text("Booting the virtual machine", "جارٍ إقلاع الآلة الافتراضية")
        }
        EngineState::Ready => language.text("Ready", "جاهز"),
        EngineState::Idle => language.text("Idle", "خامل"),
        EngineState::Paused => language.text("Paused", "متوقف مؤقتًا"),
        EngineState::Stopping => language.text("Stopping", "جارٍ الإيقاف"),
        EngineState::Failed => language.text("Failed", "فشل"),
        EngineState::Unspecified => language.text("Unknown", "غير معروف"),
    }
}

fn health_name(language: Language, state: EngineHealthState) -> &'static str {
    match state {
        EngineHealthState::Starting => language.text("Starting", "جارٍ التشغيل"),
        EngineHealthState::Ready => language.text("Ready", "جاهز"),
        EngineHealthState::Unhealthy => language.text("Unhealthy", "غير سليم"),
        EngineHealthState::Restarting => language.text("Restarting", "جارٍ إعادة التشغيل"),
        EngineHealthState::Unspecified => language.text("Unknown", "غير معروف"),
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
    format!(
        "{} {} {} {}\n",
        entry.at_unix_millis, entry.component, entry.level, entry.message
    )
}

fn log_data(entry: &LogEntry) -> Value {
    json!({
        "at_unix_millis": entry.at_unix_millis,
        "component": entry.component,
        "level": entry.level,
        "message": entry.message,
    })
}
