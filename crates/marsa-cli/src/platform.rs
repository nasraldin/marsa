//! Process handoff to the MCP server installed alongside this CLI.

use std::{env, os::unix::process::CommandExt, process::Command};

use super::Failure;

pub(super) fn launch_mcp(client_id: &str, project: &str) -> Failure {
    let Ok(cli) = env::current_exe().and_then(|path| path.canonicalize()) else {
        return Failure::McpUnavailable;
    };
    let _error = Command::new(cli.with_file_name("marsa-mcp"))
        .args(["--client-id", client_id, "--project", project])
        // Keep the bridge's diagnostics and tool descriptions in English when launched by Marsa CLI.
        .env("MARSA_MCP_LANGUAGE", "en-US")
        .exec();
    Failure::McpUnavailable
}
