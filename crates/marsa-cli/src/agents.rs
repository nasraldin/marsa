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
};
use serde_json::{Value, json};

use super::{Failure, Language, Outcome, platform};

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

pub(super) fn run(args: &ArgMatches, language: Language, yes: bool) -> Outcome {
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
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| Failure::DaemonUnavailable)
        .and_then(|runtime| runtime.block_on(execute(name, args, language, yes)));
    result.unwrap_or_else(Outcome::Failure)
}

async fn execute(
    command: &str,
    args: &ArgMatches,
    language: Language,
    yes: bool,
) -> Result<Outcome, Failure> {
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
            text.push_str(language.text("Clients\n", "العملاء\n"));
            let clients: Vec<_> = clients
                .into_iter()
                .map(|client| {
                    text.push_str(&format!(
                        "  {:?}  {:?}  {:?}\n",
                        client.client_id, client.client_name, client.project_path
                    ));
                    json!({
                        "client_id": client.client_id,
                        "client_name": client.client_name,
                        "project_path": client.project_path,
                    })
                })
                .collect();
            text.push_str(language.text("\nGrants\n", "\nالأذونات\n"));
            let grants: Vec<_> = grants
                .into_iter()
                .map(|grant| {
                    let data = grant_data(&grant);
                    text.push_str(&escape_terminal_controls(&data.to_string()));
                    text.push('\n');
                    data
                })
                .collect();
            text.push_str(language.text("\nPending requests\n", "\nالطلبات المعلّقة\n"));
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
                confirm(
                    data,
                    language,
                    language.text(
                        "Approve this request once?",
                        "هل توافق على هذا الطلب لمرة واحدة؟",
                    ),
                )
                .await?;
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
                    language.text("Request approved.", "تمت الموافقة على الطلب."),
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
                    language.text("Request denied.", "تم رفض الطلب."),
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
                    language,
                    language.text("Revoke this grant?", "هل تريد إلغاء هذا الإذن؟"),
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
                language.text("Grant revoked.", "تم إلغاء الإذن."),
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

pub(super) fn escape_terminal_controls(json: &str) -> String {
    // JSON escapes ASCII control bytes itself. Escape other invisible controls so
    // untrusted labels cannot reorder or obscure the rest of the terminal review.
    let mut escaped = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}' => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", u32::from(ch));
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

async fn confirm(data: Value, language: Language, question: &'static str) -> Result<(), Failure> {
    tokio::task::spawn_blocking(move || {
        let mut terminal = io::stderr().lock();
        let instructions = language.text("Type yes and press Enter.", "اكتب نعم ثم اضغط Enter.");
        let review =
            serde_json::to_string_pretty(&data).map_err(|_| Failure::AgentRequestFailed)?;
        let review = escape_terminal_controls(&review);
        writeln!(terminal, "{review}\n{question} {instructions}")
            .and_then(|()| terminal.flush())
            .map_err(|_| Failure::AgentReviewRequired)?;
        drop(terminal);

        let mut line = Vec::new();
        io::stdin()
            .lock()
            .take(16)
            .read_until(b'\n', &mut line)
            .map_err(|_| Failure::AgentReviewRequired)?;
        let affirmative = language.text("yes", "نعم").as_bytes();
        let answer = line
            .strip_suffix(b"\r\n")
            .or_else(|| line.strip_suffix(b"\n"));
        if answer == Some(affirmative) {
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
