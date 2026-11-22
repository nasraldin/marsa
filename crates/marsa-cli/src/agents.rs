//! Owner-only inspection and decisions for locally delegated agent sessions.

use std::{
    io::{self, BufRead, IsTerminal, Read, Write},
    path::Path,
};

use clap::ArgMatches;
use marsa_client::{
    DaemonEndpoint,
    api::{
        AgentGrant, ApproveRequest, DenyRequest, ListAgentClientsRequest, ListGrantsRequest,
        ListPendingApprovalsRequest, PendingApproval, RevokeGrantRequest,
    },
    l10n::keys,
};
use serde_json::{Value, json};

use super::{
    Failure, Outcome,
    command::{self, escape_terminal_controls},
    platform,
};

pub(super) fn mcp(args: &ArgMatches, json_output: bool) -> Failure {
    let (Some(client_id), Some(project)) = (
        args.get_one::<String>("client-id"),
        args.get_one::<String>("project"),
    ) else {
        return Failure::Usage;
    };
    if json_output || client_id.is_empty() || !Path::new(project).is_absolute() {
        return Failure::Usage;
    }
    platform::launch_mcp(client_id, project)
}

pub(super) async fn run(args: &ArgMatches, yes: bool) -> Outcome {
    let Some((name, args)) = args.subcommand() else {
        return Outcome::Failure(Failure::Usage);
    };
    let review_terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
    if (name == "approve" || name == "revoke" && !yes) && !review_terminal {
        return Outcome::Failure(if name == "approve" {
            Failure::AgentReviewRequired
        } else {
            Failure::Confirmation
        });
    }
    execute(name, args, yes)
        .await
        .unwrap_or_else(Outcome::Failure)
}

async fn execute(command: &str, args: &ArgMatches, yes: bool) -> Result<Outcome, Failure> {
    let daemon = DaemonEndpoint::discover().map_err(|_| Failure::DaemonUnavailable)?;
    let owner = daemon
        .connect()
        .await
        .map_err(|_| Failure::DaemonUnavailable)?;
    let mut agents = owner.agents();

    match command {
        "list" => {
            let clients = agents
                .list_clients(ListAgentClientsRequest {})
                .await
                .map_err(|_| Failure::AgentRequestFailed)?
                .into_inner()
                .clients;
            let grants = agents
                .list_grants(ListGrantsRequest {})
                .await
                .map_err(|_| Failure::AgentRequestFailed)?
                .into_inner()
                .grants;
            let pending = agents
                .list_pending_approvals(ListPendingApprovalsRequest {})
                .await
                .map_err(|_| Failure::AgentRequestFailed)?
                .into_inner()
                .approvals;

            let mut text = String::new();
            text.push_str(command::label(keys::CLI_AGENTS_CLIENTS));
            text.push('\n');
            let clients: Vec<_> = clients
                .into_iter()
                .map(|client| {
                    let row = format!(
                        "{:?}  {:?}  {:?}",
                        client.client_id, client.client_name, client.project_path
                    );
                    text.push_str("  ");
                    text.push_str(&escape_terminal_controls(&row));
                    text.push('\n');
                    json!({
                        "client_id": client.client_id,
                        "client_name": client.client_name,
                        "project_path": client.project_path,
                    })
                })
                .collect();
            text.push('\n');
            text.push_str(command::label(keys::CLI_AGENTS_GRANTS));
            text.push('\n');
            let grants: Vec<_> = grants
                .into_iter()
                .map(|grant| {
                    let data = grant_data(&grant);
                    text.push_str(&escape_terminal_controls(&data.to_string()));
                    text.push('\n');
                    data
                })
                .collect();
            text.push('\n');
            text.push_str(command::label(keys::CLI_AGENTS_PENDING));
            text.push('\n');
            let pending: Vec<_> = pending
                .into_iter()
                .map(|approval| {
                    let data = approval_data(approval)?;
                    text.push_str(&escape_terminal_controls(&data.to_string()));
                    text.push('\n');
                    Ok(data)
                })
                .collect::<Result<_, Failure>>()?;

            Ok(Outcome::Success {
                kind: "agents",
                text,
                data: json!({"clients": clients, "grants": grants, "pending_requests": pending}),
            })
        }
        "approve" | "deny" => {
            let id = args.get_one::<String>("request").ok_or(Failure::Usage)?;
            let request_id = request_id()?;
            if command == "approve" {
                let approval = agents
                    .list_pending_approvals(ListPendingApprovalsRequest {})
                    .await
                    .map_err(|_| Failure::AgentRequestFailed)?
                    .into_inner()
                    .approvals
                    .into_iter()
                    .find(|approval| approval.id == *id)
                    .ok_or(Failure::AgentApprovalMissing)?;
                let data = approval_data(approval)?;
                confirm(data, command::label(keys::CLI_AGENTS_APPROVE_PROMPT)).await?;
                agents
                    .approve(ApproveRequest {
                        request_id,
                        approval_id: id.clone(),
                        grant_for_project: false,
                    })
                    .await
                    .map_err(|_| Failure::AgentRequestFailed)?;
                Ok(decision(
                    "approved",
                    "approval_id",
                    id,
                    command::label(keys::CLI_AGENTS_APPROVED),
                ))
            } else {
                agents
                    .deny(DenyRequest {
                        request_id,
                        approval_id: id.clone(),
                    })
                    .await
                    .map_err(|_| Failure::AgentRequestFailed)?;
                Ok(decision(
                    "denied",
                    "approval_id",
                    id,
                    command::label(keys::CLI_AGENTS_DENIED),
                ))
            }
        }
        "revoke" => {
            let id = args.get_one::<String>("grant").ok_or(Failure::Usage)?;
            if !yes {
                let grant = agents
                    .list_grants(ListGrantsRequest {})
                    .await
                    .map_err(|_| Failure::AgentRequestFailed)?
                    .into_inner()
                    .grants
                    .into_iter()
                    .find(|grant| grant.id == *id)
                    .ok_or(Failure::AgentGrantMissing)?;
                confirm(
                    grant_data(&grant),
                    command::label(keys::CLI_AGENTS_REVOKE_PROMPT),
                )
                .await?;
            }
            agents
                .revoke_grant(RevokeGrantRequest {
                    request_id: request_id()?,
                    grant_id: id.clone(),
                })
                .await
                .map_err(|_| Failure::AgentRequestFailed)?;
            Ok(decision(
                "revoked",
                "grant_id",
                id,
                command::label(keys::CLI_AGENTS_REVOKED),
            ))
        }
        _ => Err(Failure::Usage),
    }
}

fn grant_data(grant: &AgentGrant) -> Value {
    json!({
        "id": grant.id,
        "client_id": grant.client_id,
        "project_path": grant.project_path,
        "operation": grant.operation().as_str_name(),
        "expires_at_unix_millis": grant.expires_at_unix_millis,
    })
}

fn approval_data(approval: PendingApproval) -> Result<Value, Failure> {
    let arguments: Value = serde_json::from_str(&approval.redacted_arguments_json)
        .map_err(|_| Failure::AgentRequestFailed)?;
    let operation = approval.operation();
    let tier = approval.tier();
    if !arguments.is_object()
        || operation == marsa_client::api::AgentOperation::Unspecified
        || tier == marsa_client::api::PermissionTier::Unspecified
    {
        return Err(Failure::AgentRequestFailed);
    }
    let affected: Vec<_> = approval
        .affected
        .into_iter()
        .map(|resource| {
            json!({
                "kind": resource.kind,
                "name": resource.name,
                "size_bytes": resource.size_bytes,
                "related_containers": resource.related_containers,
            })
        })
        .collect();
    Ok(json!({
        "id": approval.id,
        "client_id": approval.client_id,
        "client_name": approval.client_name,
        "project_path": approval.project_path,
        "operation": operation.as_str_name(),
        "tier": tier.as_str_name(),
        "redacted_arguments": arguments,
        "affected": affected,
        "expires_at_unix_millis": approval.expires_at_unix_millis,
        "can_grant_for_project": approval.can_grant_for_project,
    }))
}

async fn confirm(data: Value, question: &'static str) -> Result<(), Failure> {
    tokio::task::spawn_blocking(move || {
        let mut terminal = io::stderr().lock();
        // These tokens are accepted input, independent of catalog wording.
        let affirmative = "yes";
        let instructions = command::message(
            keys::CLI_AGENTS_CONFIRM_INSTRUCTIONS,
            &[
                ("answer", &command::technical(affirmative)),
                ("enter", &command::technical("Enter")),
            ],
        );
        let review =
            serde_json::to_string_pretty(&data).map_err(|_| Failure::AgentRequestFailed)?;
        for row in review.lines() {
            writeln!(terminal, "{}", escape_terminal_controls(row))
                .map_err(|_| Failure::AgentReviewRequired)?;
        }
        writeln!(terminal, "{question} {instructions}")
            .and_then(|()| terminal.flush())
            .map_err(|_| Failure::AgentReviewRequired)?;
        drop(terminal);

        let mut line = Vec::new();
        io::stdin()
            .lock()
            .take(16)
            .read_until(b'\n', &mut line)
            .map_err(|_| Failure::AgentReviewRequired)?;
        let answer = line
            .strip_suffix(b"\r\n")
            .or_else(|| line.strip_suffix(b"\n"));
        if answer == Some(affirmative.as_bytes()) {
            Ok(())
        } else {
            Err(Failure::AgentReviewDeclined)
        }
    })
    .await
    .map_err(|_| Failure::AgentReviewRequired)?
}

fn decision(action: &str, id_key: &str, id: &str, text: &str) -> Outcome {
    Outcome::Success {
        kind: "agent_decision",
        text: format!("{text}\n"),
        data: json!({"decision": action, id_key: id}),
    }
}

fn request_id() -> Result<String, Failure> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Failure::AgentRequestFailed)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
