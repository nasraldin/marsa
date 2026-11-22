//! Marsa command-line interface and stable response handling.

mod command;
#[cfg(unix)]
mod disk;
#[cfg(unix)]
mod engine;
#[cfg(unix)]
mod maintenance;
#[cfg(unix)]
mod network;
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
use serde_json::json;

use command::{Language, build};

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
        }
    }

    fn message(self, language: Language) -> &'static str {
        match self {
            Self::Unavailable => {
                marsa_client::l10n::keys::CLI_NOT_AVAILABLE.text(language.locale())
            }
            Self::Usage => language.text(
                "Invalid invocation. Run marsa --help for usage.",
                "استدعاء غير صالح. شغّل marsa --help لعرض طريقة الاستخدام.",
            ),
            Self::Confirmation => language.text(
                "This operation requires --yes when run non-interactively.",
                "تتطلب هذه العملية الخيار --yes عند تشغيلها دون تفاعل.",
            ),
            #[cfg(unix)]
            Self::ConfirmationDeclined => language.text(
                "Reset canceled. The confirmation did not match.",
                "أُلغيَت إعادة الضبط لأن عبارة التأكيد غير مطابقة.",
            ),
            Self::Connection => language.text(
                "Could not connect to the Marsa daemon. Open Marsa and try again.",
                "تعذّر الاتصال بخادم مرسى. افتح التطبيق ثم أعد المحاولة.",
            ),
            Self::Home => language.text(
                "Could not resolve the socket under the current user's home directory.",
                "تعذّر تحديد موقع المقبس في المجلد الرئيسي للمستخدم الحالي.",
            ),
            Self::Root => language.text(
                "Run Marsa as an unprivileged user.",
                "شغّل مرسى بحساب مستخدم عادي.",
            ),
            Self::Incompatible => language.text(
                "The daemon API is incompatible with this version of the Marsa CLI.",
                "واجهة الخادم غير متوافقة مع إصدار سطر أوامر مرسى الحالي.",
            ),
            Self::Operation => language.text(
                "The Marsa operation failed. Run marsa status or check marsa logs for details.",
                "فشلت عملية مرسى. شغّل marsa status أو اطّلع على marsa logs لمعرفة التفاصيل.",
            ),
            Self::BindResolution => language.text(
                "Could not answer the bind request. Run marsa share approvals to check whether it is still pending.",
                "تعذّر الرد على طلب الوصل. شغّل marsa share approvals للتحقق من أن الطلب ما زال معلّقًا.",
            ),
            Self::CleanupApplyUnavailable => language.text(
                "Cleanup apply is unavailable. Run marsa disk cleanup --dry-run to preview.",
                "تنفيذ التنظيف غير متاح. شغّل marsa disk cleanup --dry-run للمعاينة.",
            ),
            Self::CleanupPreview => language.text(
                "Could not preview unused storage. Disk cleanup preview requires a running engine.",
                "تعذّرت معاينة المساحة غير المستخدمة. تتطلب المعاينة محركًا قيد التشغيل.",
            ),
            Self::Reclaim => language.text(
                "Could not reclaim disk space. Reclaim requires a running VM.",
                "تعذّر استرداد مساحة القرص. تتطلب العملية تشغيل الآلة الافتراضية.",
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
    Data {
        document: serde_json::Value,
        text: String,
    },
    Streamed,
    Rejected(String),
    Failure(Failure),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
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
    let outcome = execute(args, language, interactive, json).await;
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

async fn execute(
    args: Vec<OsString>,
    language: Language,
    interactive: bool,
    json: bool,
) -> Outcome {
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
    let yes = matches.get_flag("yes");
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
        return settings::run(args, language).await;
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
        Outcome::Rejected(_) => 1,
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
            Outcome::Rejected(message) => json!({
                "schema_version": 1,
                "kind": "error",
                "code": "request_rejected",
                "message": message,
            }),
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
            Outcome::Rejected(message) => writeln!(io::stderr().lock(), "{message}")?,
            Outcome::Help(text) | Outcome::Data { text, .. } => {
                write!(io::stdout().lock(), "{text}")?
            }
            Outcome::Completion { script, .. } => write!(io::stdout().lock(), "{script}")?,
            Outcome::Failure(failure) => {
                writeln!(io::stderr().lock(), "{}", failure.message(language))?
            }
        }
    }
    Ok(code)
}
