//! Present and answer exact bind approvals from the local daemon.

use std::fmt::Write as _;

use clap::ArgMatches;
use marsa_client::{
    DaemonEndpoint, MarsaClient,
    api::{BindApprovalDecision, ResolveBindRequest, daemon_event::Body},
    l10n::keys,
};
use serde_json::{Value, json};

use crate::{
    Failure, Outcome, command,
    engine::{connection_failure, request_id},
};

pub(super) async fn run(args: &ArgMatches) -> Option<Outcome> {
    let command = match args.subcommand()? {
        ("approvals", _) => Ok(Command::List),
        ("resolve", args) => resolution(args),
        _ => return None,
    };
    Some(match command {
        Ok(command) => command.run().await.unwrap_or_else(Outcome::Failure),
        Err(error) => Outcome::Failure(error),
    })
}

enum Command<'a> {
    List,
    Resolve {
        id: &'a str,
        decision_name: &'a str,
        decision: BindApprovalDecision,
    },
}

fn resolution(args: &ArgMatches) -> Result<Command<'_>, Failure> {
    let id = args.get_one::<String>("id").ok_or(Failure::Usage)?;
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Failure::Usage);
    }
    let decision_name = args
        .get_one::<String>("decision")
        .map(String::as_str)
        .ok_or(Failure::Usage)?;
    let decision = match decision_name {
        "once" => BindApprovalDecision::Once,
        "always" => BindApprovalDecision::Persist,
        "deny" => BindApprovalDecision::Deny,
        _ => return Err(Failure::Usage),
    };
    Ok(Command::Resolve {
        id,
        decision_name,
        decision,
    })
}

impl Command<'_> {
    async fn run(self) -> Result<Outcome, Failure> {
        let client = DaemonEndpoint::discover()
            .map_err(connection_failure)?
            .connect()
            .await
            .map_err(connection_failure)?;
        match self {
            Self::List => list(&client).await,
            Self::Resolve {
                id,
                decision_name,
                decision,
            } => {
                let result = client
                    .shares()
                    .resolve_bind(ResolveBindRequest {
                        request_id: request_id()?,
                        approval_id: id.to_owned(),
                        decision: decision.into(),
                    })
                    .await
                    .map_err(|_| Failure::BindResolution)?
                    .into_inner();
                let status = if result.requires_restart {
                    command::label(keys::CLI_SHARE_BIND_APPROVED)
                } else {
                    command::label(keys::CLI_SHARE_BIND_DENIED)
                };
                Ok(Outcome::Data {
                    document: json!({
                        "schema_version": 1,
                        "kind": "bind_resolution",
                        "id": id,
                        "decision": decision_name,
                        "requires_restart": result.requires_restart,
                    }),
                    text: format!("{status}\n"),
                })
            }
        }
    }
}

async fn list(client: &MarsaClient) -> Result<Outcome, Failure> {
    let mut subscription = client.subscribe_events();
    let event = subscription.next().await.map_err(connection_failure)?;
    let Some(Body::Snapshot(snapshot)) = event.body else {
        return Err(Failure::Operation);
    };
    let approvals = snapshot
        .bind_approvals
        .ok_or(Failure::Incompatible)?
        .approvals;
    let mut text = String::new();
    if approvals.is_empty() {
        let _ = writeln!(text, "{}", command::label(keys::CLI_SHARE_NO_APPROVALS),);
    }
    let mut records = Vec::<Value>::with_capacity(approvals.len());
    for approval in approvals {
        let _ = writeln!(text, "{}", command::technical(&approval.id));
        for directory in &approval.directories {
            // Escape bidi override and terminal controls in user-supplied file names.
            let _ = writeln!(text, "  {}", command::technical(directory));
        }
        records.push(json!({
            "id": approval.id,
            "directories": approval.directories,
            "expires_at_unix_millis": approval.expires_at_unix_millis,
        }));
    }
    Ok(Outcome::Data {
        document: json!({
            "schema_version": 1,
            "kind": "bind_approvals",
            "approvals": records,
        }),
        text,
    })
}
