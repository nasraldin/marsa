//! Local MCP stdio bridge. All reads use the daemon's delegated AgentOps endpoint.

#[cfg(unix)]
mod tools;
#[cfg(unix)]
mod wire;

use std::process::ExitCode;

#[cfg(unix)]
use marsa_client::{
    AgentClient, DaemonEndpoint,
    api::{AgentOperation, AgentOperationResponse, OpenAgentSessionRequest},
};

#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = match error {
                StartError::Arguments => text(
                    "Configure a trusted local launch with marsa-mcp --client-id ID --project /absolute/project.",
                    "هيّئ تشغيلًا محليًا موثوقًا باستخدام marsa-mcp --client-id ID --project /absolute/project.",
                ),
                StartError::Session => text(
                    "Marsa could not authenticate the local MCP session.",
                    "تعذّر على مارسا مصادقة جلسة MCP المحلية.",
                ),
                StartError::Transport => text(
                    "Marsa could not serve MCP over standard input and output.",
                    "تعذّر على مارسا خدمة MCP عبر الإدخال والإخراج القياسيين.",
                ),
            };
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(unix))]
fn main() -> ExitCode {
    ExitCode::FAILURE
}

#[cfg(unix)]
enum StartError {
    Arguments,
    Session,
    Transport,
}

#[cfg(unix)]
async fn run() -> Result<(), StartError> {
    let mut args = std::env::args_os().skip(1);
    let Some(flag) = args.next() else {
        return Err(StartError::Arguments);
    };
    if flag != "--client-id" {
        return Err(StartError::Arguments);
    }
    let client_id = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(StartError::Arguments)?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--project")) {
        return Err(StartError::Arguments);
    }
    let project_path = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(StartError::Arguments)?;
    if args.next().is_some() {
        return Err(StartError::Arguments);
    }
    let owner = DaemonEndpoint::discover()
        .map_err(|_| StartError::Session)?
        .connect()
        .await
        .map_err(|_| StartError::Session)?;
    let principal = owner
        .delegate_agent(OpenAgentSessionRequest {
            request_id: request_id().map_err(|_| StartError::Session)?,
            client_name: client_id.clone(),
            client_id,
            project_path,
        })
        .await
        .map_err(|_| StartError::Session)?;
    drop(owner);
    wire::serve(Local(principal), tokio::io::stdin(), tokio::io::stdout())
        .await
        .map_err(|_| StartError::Transport)
}

#[cfg(unix)]
fn text<'a>(english: &'a str, arabic: &'a str) -> &'a str {
    if std::env::var_os("MARSA_MCP_LANGUAGE").is_some_and(|value| value == "en-US") {
        return english;
    }
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty());
    if locale
        .as_deref()
        .and_then(|value| value.split(['_', '-', '.', '@']).next())
        == Some("ar")
    {
        arabic
    } else {
        english
    }
}

#[cfg(unix)]
#[derive(Clone)]
struct Local(AgentClient);

#[cfg(unix)]
impl wire::Backend for Local {
    async fn execute(
        &self,
        operation: AgentOperation,
        arguments: String,
    ) -> Result<AgentOperationResponse, ()> {
        self.0
            .clone()
            .execute(request_id()?, operation, arguments)
            .await
            .map_err(|_| ())
    }
}

#[cfg(unix)]
fn request_id() -> Result<String, ()> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    for (index, byte) in bytes.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").map_err(|_| ())?;
    }
    Ok(id)
}
