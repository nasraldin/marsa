//! Manage persistent owner-approved folders using the typed Shares service.

use std::fmt::Write as _;

use clap::ArgMatches;
use marsa_client::{
    DaemonEndpoint,
    api::{AddShareRequest, ConfigurationIssue, ListSharesRequest, RemoveShareRequest, Share},
    l10n::keys,
};
use serde_json::json;

use crate::{
    Failure, Outcome,
    command::Language,
    configuration_feedback::rejection,
    engine::{connection_failure, request_id},
    settings::saved,
};

pub(super) async fn run(args: &ArgMatches, language: Language) -> Option<Outcome> {
    let (name, args) = args.subcommand()?;
    if !matches!(name, "list" | "add" | "remove") {
        return None;
    }
    Some(
        apply(name, args, language)
            .await
            .unwrap_or_else(Outcome::Failure),
    )
}

async fn apply(name: &str, args: &ArgMatches, language: Language) -> Result<Outcome, Failure> {
    let client = DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)?;
    let mut shares = client.shares();
    match name {
        "list" => {
            let response = shares.list(ListSharesRequest {}).await;
            let entries = match response {
                Ok(response) => response.into_inner().shares,
                Err(error) => {
                    return Ok(rejection(
                        language,
                        ConfigurationIssue::from_status(&error),
                        (client.api_minor() < ConfigurationIssue::API_MINOR)
                            .then(|| error.message()),
                    ));
                }
            };
            let mut text = String::new();
            if entries.is_empty() {
                let _ = writeln!(text, "{}", language.label(keys::CLI_SHARE_FOLDERS_EMPTY));
            }
            for entry in &entries {
                let _ = writeln!(text, "{}", language.technical(&entry.host_path));
            }
            Ok(Outcome::Data {
                text,
                document: json!({
                    "schema_version": 1,
                    "kind": "shares",
                    "shares": entries.iter().map(share_json).collect::<Vec<_>>(),
                }),
            })
        }
        "add" => {
            let path = args.get_one::<String>("path").ok_or(Failure::Usage)?;
            let response = shares
                .add(AddShareRequest {
                    request_id: request_id()?,
                    host_path: path.to_owned(),
                    persist: true,
                    read_only: false,
                    approval_id: None,
                })
                .await;
            match response {
                Ok(response) => {
                    let share = response.into_inner();
                    Ok(Outcome::Data {
                        text: format!("{}\n", saved(language, share.requires_restart)),
                        document: json!({"schema_version": 1, "kind": "share_added", "share": share_json(&share)}),
                    })
                }
                Err(error) => Ok(rejection(
                    language,
                    ConfigurationIssue::from_status(&error),
                    (client.api_minor() < ConfigurationIssue::API_MINOR).then(|| error.message()),
                )),
            }
        }
        "remove" => {
            let id = args.get_one::<String>("share").ok_or(Failure::Usage)?;
            let response = shares
                .remove(RemoveShareRequest {
                    request_id: request_id()?,
                    share_id: id.to_owned(),
                })
                .await;
            match response {
                Ok(response) => {
                    let restart = response.into_inner().requires_restart;
                    Ok(Outcome::Data {
                        text: format!("{}\n", saved(language, restart)),
                        document: json!({
                            "schema_version": 1, "kind": "share_removed", "id": id,
                            "requires_restart": restart,
                        }),
                    })
                }
                Err(error) => Ok(rejection(
                    language,
                    ConfigurationIssue::from_status(&error),
                    (client.api_minor() < ConfigurationIssue::API_MINOR).then(|| error.message()),
                )),
            }
        }
        _ => Err(Failure::Usage),
    }
}

fn share_json(share: &Share) -> serde_json::Value {
    json!({
        "id": share.id,
        "path": share.host_path,
        "read_only": share.read_only,
        "requires_restart": share.requires_restart,
    })
}
