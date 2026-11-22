//! Marsa command-line interface and stable response handling.

#[cfg(unix)]
mod agents;
mod command;
#[cfg(unix)]
mod platform;

use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use clap::{ArgMatches, error::ErrorKind};
use clap_complete::Shell;
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;

use command::{Language, build};

#[derive(Clone, Copy)]
enum Failure {
    Unavailable,
    Usage,
    Confirmation,
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
            Self::Unavailable => 1,
            Self::Usage => 2,
            Self::Confirmation => 3,
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
            Self::Unavailable => "not_available",
            Self::Usage => "invalid_invocation",
            Self::Confirmation => "confirmation_required",
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
            Self::Unavailable => language.text(
                "This command is not available yet.",
                "هذا الأمر غير متاح حاليًا.",
            ),
            Self::Usage => language.text(
                "Invalid invocation. Run marsa --help for usage.",
                "استدعاء غير صالح. شغّل marsa --help لعرض طريقة الاستخدام.",
            ),
            Self::Confirmation => language.text(
                "This operation requires --yes when run non-interactively.",
                "تتطلب هذه العملية الخيار --yes عند تشغيلها دون تفاعل.",
            ),
            #[cfg(unix)]
            Self::DaemonUnavailable => language.text(
                "Could not connect to the local Marsa daemon.",
                "تعذّر الاتصال بخادم مرسى المحلي.",
            ),
            #[cfg(unix)]
            Self::AgentRequestFailed => language.text(
                "Marsa could not complete the agent command.",
                "تعذّر على مرسى إكمال أمر الوكيل.",
            ),
            #[cfg(unix)]
            Self::AgentApprovalMissing => language.text(
                "No pending approval matches that ID.",
                "لا توجد موافقة معلّقة تطابق هذا المعرّف.",
            ),
            #[cfg(unix)]
            Self::AgentGrantMissing => language.text(
                "No active grant matches that ID.",
                "لا يوجد إذن نشط يطابق هذا المعرّف.",
            ),
            #[cfg(unix)]
            Self::AgentReviewRequired => language.text(
                "Approving an agent request requires reviewing and confirming it in a terminal.",
                "تتطلب الموافقة على طلب الوكيل مراجعته وتأكيده في الطرفية.",
            ),
            #[cfg(unix)]
            Self::AgentReviewDeclined => {
                language.text("The confirmation was declined.", "تم رفض التأكيد.")
            }
            #[cfg(unix)]
            Self::McpUnavailable => language.text(
                "Could not start marsa-mcp beside the Marsa CLI.",
                "تعذّر تشغيل marsa-mcp بجانب واجهة سطر أوامر مرسى.",
            ),
        }
    }
}

enum Outcome {
    Help(String),
    Completion {
        shell: Shell,
        script: String,
    },
    Failure(Failure),
    #[cfg(unix)]
    Success {
        kind: &'static str,
        text: String,
        data: Value,
    },
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().collect();
    let json = args
        .iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    let language = if json {
        Language::English
    } else {
        Language::current()
    };
    let interactive = io::stdin().is_terminal() && !json;
    let outcome = execute(args, language, interactive, json);
    match render(outcome, language, json) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if !json {
                let message = language.text("Could not write the response", "تعذّرت كتابة الاستجابة");
                let _ = writeln!(io::stderr().lock(), "{message} ({error})");
            }
            ExitCode::FAILURE
        }
    }
}

fn execute(args: Vec<OsString>, language: Language, interactive: bool, _json: bool) -> Outcome {
    let mut cli = build(language);
    if args.len() == 1 || args.len() == 2 && args[1] == "--json" {
        return Outcome::Help(cli.render_help().to_string());
    }
    let matches = match cli.try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            return Outcome::Help(error.to_string());
        }
        Err(_) => return Outcome::Failure(Failure::Usage),
    };
    let Some((name, args)) = matches.subcommand() else {
        return Outcome::Failure(Failure::Usage);
    };
    if destructive(name, args) && !interactive && !matches.get_flag("yes") {
        return Outcome::Failure(Failure::Confirmation);
    }
    match name {
        "completion" => completions(args),
        #[cfg(unix)]
        "agents" => agents::run(args, Language::current(), matches.get_flag("yes")),
        #[cfg(unix)]
        "mcp" => Outcome::Failure(agents::mcp(args, _json)),
        _ => Outcome::Failure(Failure::Unavailable),
    }
}

fn destructive(name: &str, args: &ArgMatches) -> bool {
    name == "reset"
        || name == "disk"
            && args
                .subcommand_matches("cleanup")
                .is_some_and(|cleanup| !cleanup.get_flag("dry-run"))
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
        _ => 0,
    };
    if json_output {
        let body = match outcome {
            Outcome::Help(text) => json!({"schema_version": 1, "kind": "help", "text": text}),
            Outcome::Completion { shell, script } => {
                json!({"schema_version": 1, "kind": "completion", "shell": shell.to_string(), "script": script})
            }
            #[cfg(unix)]
            Outcome::Success { kind, data, .. } => {
                json!({"schema_version": 1, "kind": kind, "data": data})
            }
            Outcome::Failure(failure) => json!({
                "schema_version": 1,
                "kind": "error",
                "code": failure.key(),
                "message": failure.message(Language::English),
            }),
        };
        let body = body.to_string();
        #[cfg(unix)]
        let body = agents::escape_terminal_controls(&body);
        writeln!(io::stdout().lock(), "{body}")?;
    } else {
        match outcome {
            Outcome::Help(text) => write!(io::stdout().lock(), "{text}")?,
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
