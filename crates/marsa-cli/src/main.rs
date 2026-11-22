//! Marsa command-line interface and stable response handling.

#[cfg(unix)]
mod agents;
mod command;
#[cfg(unix)]
mod configuration_feedback;
#[cfg(unix)]
mod disk;
#[cfg(unix)]
mod engine;
#[cfg(unix)]
mod maintenance;
#[cfg(unix)]
mod network;
#[cfg(unix)]
mod platform;
#[cfg(unix)]
mod settings;
#[cfg(unix)]
mod share;
#[cfg(unix)]
mod share_folders;

use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use clap::{ArgMatches, error::ErrorKind};
use clap_complete::Shell;
use marsa_client::l10n::keys;
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;

use command::{Language, build, explicit_language};

#[derive(Clone, Copy)]
enum Failure {
    Unavailable,
    Usage,
    Confirmation,
    #[cfg(unix)]
    ConfirmationDeclined,
    Connection,
    Home,
    Root,
    Incompatible,
    Operation,
    BindResolution,
    Reclaim,
    CleanupApplyUnavailable,
    CleanupPreview,
    #[cfg(unix)]
    DaemonUnavailable,
    #[cfg(unix)]
    AgentRequestFailed,
    #[cfg(unix)]
    AgentApprovalMissing,
    #[cfg(unix)]
    AgentGrantMissing,
    #[cfg(unix)]
    AgentReviewRequired,
    #[cfg(unix)]
    AgentReviewDeclined,
    #[cfg(unix)]
    McpUnavailable,
}

impl Failure {
    fn code(self) -> u8 {
        match self {
            Self::Unavailable
            | Self::Operation
            | Self::BindResolution
            | Self::Reclaim
            | Self::CleanupApplyUnavailable
            | Self::CleanupPreview => 1,
            Self::Usage => 2,
            Self::Confirmation => 3,
            #[cfg(unix)]
            Self::ConfirmationDeclined => 3,
            Self::Connection | Self::Home | Self::Root => 4,
            Self::Incompatible => 5,
            #[cfg(unix)]
            Self::AgentReviewRequired | Self::AgentReviewDeclined => 3,
            #[cfg(unix)]
            Self::DaemonUnavailable
            | Self::AgentRequestFailed
            | Self::AgentApprovalMissing
            | Self::AgentGrantMissing
            | Self::McpUnavailable => 1,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Unavailable | Self::CleanupApplyUnavailable => "not_available",
            Self::Usage => "invalid_invocation",
            Self::Confirmation => "confirmation_required",
            #[cfg(unix)]
            Self::ConfirmationDeclined => "confirmation_required",
            Self::Connection => "connection_failed",
            Self::Home => "home_unavailable",
            Self::Root => "unprivileged_user_required",
            Self::Incompatible => "incompatible_daemon",
            Self::Operation | Self::Reclaim | Self::CleanupPreview => "operation_failed",
            Self::BindResolution => "bind_resolution_failed",
            #[cfg(unix)]
            Self::DaemonUnavailable => "daemon_unavailable",
            #[cfg(unix)]
            Self::AgentRequestFailed => "agent_request_failed",
            #[cfg(unix)]
            Self::AgentApprovalMissing => "agent_approval_missing",
            #[cfg(unix)]
            Self::AgentGrantMissing => "agent_grant_missing",
            #[cfg(unix)]
            Self::AgentReviewRequired => "confirmation_required",
            #[cfg(unix)]
            Self::AgentReviewDeclined => "confirmation_declined",
            #[cfg(unix)]
            Self::McpUnavailable => "mcp_unavailable",
        }
    }

    fn message(self, language: Language) -> &'static str {
        match self {
            Self::Unavailable => language.label(keys::CLI_NOT_AVAILABLE),
            Self::Usage => language.label(keys::CLI_INVALID_INVOCATION),
            Self::Confirmation => language.label(keys::CLI_CONFIRMATION_REQUIRED),
            #[cfg(unix)]
            Self::ConfirmationDeclined => language.label(keys::CLI_CONFIRMATION_DECLINED),
            Self::Connection => language.label(keys::CLI_CONNECTION_FAILED),
            Self::Home => language.label(keys::CLI_HOME_UNAVAILABLE),
            Self::Root => language.label(keys::CLI_UNPRIVILEGED_USER_REQUIRED),
            Self::Incompatible => language.label(keys::CLI_INCOMPATIBLE_DAEMON),
            Self::Operation => language.label(keys::CLI_OPERATION_FAILED),
            Self::BindResolution => language.label(keys::CLI_BIND_RESOLUTION_FAILED),
            Self::CleanupApplyUnavailable => language.label(keys::CLI_CLEANUP_APPLY_UNAVAILABLE),
            Self::CleanupPreview => language.label(keys::CLI_CLEANUP_PREVIEW),
            Self::Reclaim => language.label(keys::CLI_RECLAIM),
            #[cfg(unix)]
            Self::DaemonUnavailable => language.label(keys::CLI_DAEMON_UNAVAILABLE),
            #[cfg(unix)]
            Self::AgentRequestFailed => language.label(keys::CLI_AGENT_REQUEST_FAILED),
            #[cfg(unix)]
            Self::AgentApprovalMissing => language.label(keys::CLI_AGENT_APPROVAL_MISSING),
            #[cfg(unix)]
            Self::AgentGrantMissing => language.label(keys::CLI_AGENT_GRANT_MISSING),
            #[cfg(unix)]
            Self::AgentReviewRequired => language.label(keys::CLI_AGENT_REVIEW_REQUIRED),
            #[cfg(unix)]
            Self::AgentReviewDeclined => language.label(keys::CLI_AGENT_REVIEW_DECLINED),
            #[cfg(unix)]
            Self::McpUnavailable => language.label(keys::CLI_MCP_UNAVAILABLE),
        }
    }
}

enum Outcome {
    Help(String),
    Completion {
        shell: Shell,
        script: String,
    },
    Data {
        document: serde_json::Value,
        text: String,
    },
    Streamed,
    Rejected {
        text: String,
        message: String,
    },
    Failure(Failure),
    #[cfg(unix)]
    Success {
        kind: &'static str,
        text: String,
        data: Value,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().collect();
    let json = args
        .iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    let explicit = explicit_language(&args);
    let language = explicit.ok().flatten().unwrap_or_else(Language::current);
    let interactive = io::stdin().is_terminal() && !json;
    let outcome = if explicit.is_err() {
        Outcome::Failure(Failure::Usage)
    } else {
        execute(args, language, interactive, json).await
    };
    match render(outcome, language, json) {
        Ok(code) => ExitCode::from(code),
        Err(_) => {
            if !json {
                let message = language.label(keys::CLI_WRITE_RESPONSE_FAILED);
                let _ = writeln!(io::stderr().lock(), "{message}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn execute(
    args: Vec<OsString>,
    display_language: Language,
    interactive: bool,
    json: bool,
) -> Outcome {
    let language = if json {
        Language::English
    } else {
        display_language
    };
    let root_help = args.last().is_none_or(|arg| arg != "--");
    let matches = match build(language).try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            return Outcome::Help(error.to_string());
        }
        Err(_) => return Outcome::Failure(Failure::Usage),
    };
    let yes = matches.get_flag("yes");
    let Some((name, args)) = matches.subcommand() else {
        return if yes || !root_help {
            Outcome::Failure(Failure::Usage)
        } else {
            Outcome::Help(build(language).render_help().to_string())
        };
    };
    if name == "reset" && !interactive && !yes {
        return Outcome::Failure(Failure::Confirmation);
    }
    if name == "completion" {
        return completions(args);
    }
    if name == "disk"
        && let Some(cleanup) = args.subcommand_matches("cleanup")
        && !cleanup.get_flag("dry-run")
    {
        return Outcome::Failure(Failure::CleanupApplyUnavailable);
    }
    #[cfg(unix)]
    if name == "network" {
        let result = match args.subcommand() {
            Some(("status", _)) => network::status(language).await,
            Some(("mode", mode)) => {
                let requested = mode.get_one::<String>("mode").map(String::as_str);
                network::mode(requested, language).await
            }
            _ => Err(Failure::Usage),
        };
        return result.unwrap_or_else(Outcome::Failure);
    }
    #[cfg(unix)]
    if name == "config" {
        return settings::run(args, language, !json && io::stdout().is_terminal()).await;
    }
    #[cfg(unix)]
    if name == "share"
        && let Some(outcome) = share::run(args, language).await
    {
        return outcome;
    }
    #[cfg(unix)]
    if name == "share"
        && let Some(outcome) = share_folders::run(args, language).await
    {
        return outcome;
    }
    #[cfg(unix)]
    if name == "disk" {
        let result = match args.subcommand() {
            Some(("usage", _)) => disk::usage(language).await,
            Some(("reclaim", _)) => disk::reclaim(language).await,
            Some(("cleanup", cleanup)) => disk::preview_cleanup(cleanup, language).await,
            _ => Err(Failure::Unavailable),
        };
        return result.unwrap_or_else(Outcome::Failure);
    }
    #[cfg(unix)]
    if let Some(outcome) = maintenance::run(name, args, language, yes).await {
        return outcome;
    }
    #[cfg(unix)]
    match name {
        "agents" => return agents::run(args, display_language, yes).await,
        "mcp" => return Outcome::Failure(agents::mcp(args, json)),
        _ => {}
    }
    #[cfg(unix)]
    if let Some(outcome) = engine::run(name, args, language, json).await {
        return outcome;
    }
    Outcome::Failure(Failure::Unavailable)
}

fn completions(args: &ArgMatches) -> Outcome {
    let shell = match args.get_one::<String>("shell").map(String::as_str) {
        Some("bash") => Shell::Bash,
        Some("zsh") => Shell::Zsh,
        Some("fish") => Shell::Fish,
        _ => return Outcome::Failure(Failure::Usage),
    };
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut build(Language::English), "marsa", &mut script);
    match String::from_utf8(script) {
        Ok(script) => Outcome::Completion { shell, script },
        Err(_) => Outcome::Failure(Failure::Unavailable),
    }
}

fn render(outcome: Outcome, language: Language, json_output: bool) -> io::Result<u8> {
    let code = match outcome {
        Outcome::Failure(failure) => failure.code(),
        Outcome::Rejected { .. } => 1,
        _ => 0,
    };
    if json_output {
        let body = match outcome {
            Outcome::Help(text) => json!({"schema_version": 1, "kind": "help", "text": text}),
            Outcome::Completion { shell, script } => {
                json!({"schema_version": 1, "kind": "completion", "shell": shell.to_string(), "script": script})
            }
            Outcome::Streamed => return Ok(code),
            Outcome::Data { document, .. } => document,
            Outcome::Rejected { message, .. } => json!({
                "schema_version": 1,
                "kind": "error",
                "code": "request_rejected",
                "message": message,
            }),
            #[cfg(unix)]
            Outcome::Success { kind, data, .. } => {
                let body = json!({"schema_version": 1, "kind": kind, "data": data});
                let body = command::escape_terminal_controls(&body.to_string());
                writeln!(io::stdout().lock(), "{body}")?;
                return Ok(code);
            }
            Outcome::Failure(failure) => json!({
                "schema_version": 1,
                "kind": "error",
                "code": failure.key(),
                "message": failure.message(Language::English),
            }),
        };
        writeln!(io::stdout().lock(), "{body}")?;
    } else {
        match outcome {
            Outcome::Streamed => (),
            Outcome::Rejected { text, .. } => writeln!(io::stderr().lock(), "{text}")?,
            Outcome::Help(text) | Outcome::Data { text, .. } => {
                write!(io::stdout().lock(), "{text}")?
            }
            Outcome::Completion { script, .. } => write!(io::stdout().lock(), "{script}")?,
            #[cfg(unix)]
            Outcome::Success { text, .. } => write!(io::stdout().lock(), "{text}")?,
            Outcome::Failure(failure) => {
                writeln!(io::stderr().lock(), "{}", failure.message(language))?
            }
        }
    }
    Ok(code)
}
