//! Report the daemon's sparse image and guest filesystem observations.

use std::fmt::Write as _;

use clap::ArgMatches;
use marsa_client::{
    ClientError, DaemonEndpoint, MarsaClient,
    api::{
        DiskCategory, DiskReclaimRequest, DiskUsageRequest, DiskUsageResponse, PrunePreviewRequest,
        PrunePreviewResponse,
    },
    l10n::keys,
};
use serde_json::{Value, json};

use crate::{Failure, Outcome, command, engine::request_id};

pub(super) async fn usage() -> Result<Outcome, Failure> {
    let client = connect().await?;
    let usage = client
        .disk()
        .usage(DiskUsageRequest {})
        .await
        .map_err(|_| Failure::Operation)?
        .into_inner();
    let (document, text) = report(&usage);
    Ok(Outcome::Data { document, text })
}

pub(super) async fn reclaim() -> Result<Outcome, Failure> {
    let client = connect().await?;
    let result = client
        .disk()
        .reclaim(DiskReclaimRequest {
            request_id: request_id().map_err(|_| Failure::Reclaim)?,
        })
        .await
        .map_err(|_| Failure::Reclaim)?
        .into_inner();
    let usage = result.usage.ok_or(Failure::Reclaim)?;
    let (usage, details) = report(&usage);
    let host = command::message(
        keys::CLI_DISK_RECLAIM_HOST,
        &[(
            "bytes",
            &command::technical(&result.host_reclaimed_bytes.to_string()),
        )],
    );
    let guest = command::message(
        keys::CLI_DISK_RECLAIM_GUEST,
        &[(
            "bytes",
            &command::technical(&result.guest_trimmed_bytes.to_string()),
        )],
    );
    Ok(Outcome::Data {
        text: format!("{host}\n{guest}\n{details}"),
        document: json!({
            "schema_version": 1,
            "kind": "disk_reclaim",
            "guest_trimmed_bytes": result.guest_trimmed_bytes,
            "host_reclaimed_bytes": result.host_reclaimed_bytes,
            "usage": usage,
        }),
    })
}

pub(super) async fn preview_cleanup(args: &ArgMatches) -> Result<Outcome, Failure> {
    let categories = if args.get_flag("include-volumes") {
        [
            DiskCategory::Containers,
            DiskCategory::Images,
            DiskCategory::Volumes,
            DiskCategory::BuildCache,
        ]
        .into_iter()
        .map(|category| category as i32)
        .collect()
    } else {
        Vec::new()
    };
    let preview = connect()
        .await?
        .disk()
        .prune_preview(PrunePreviewRequest {
            categories,
            volume_names: Vec::new(),
        })
        .await
        .map_err(|_| Failure::CleanupPreview)?
        .into_inner();
    Ok(cleanup_report(&preview))
}

fn cleanup_report(preview: &PrunePreviewResponse) -> Outcome {
    let heading = command::label(keys::CLI_DISK_PREVIEW_HEADING);
    let unavailable = command::label(keys::CLI_DISK_PREVIEW_ESTIMATE_UNAVAILABLE);
    let mut text = format!("{heading}\n");
    let mut last = None;
    let mut candidates = Vec::with_capacity(preview.candidates.len());
    for row in &preview.candidates {
        let category = row.category();
        if last != Some(category) {
            let _ = writeln!(text, "{}", category_name(category));
            last = Some(category);
        }
        let name = if row.name == row.id {
            row.id.clone()
        } else {
            format!("{} ({})", row.name, row.id)
        };
        let estimate = row.size_bytes.map_or_else(
            || unavailable.to_owned(),
            |size| {
                command::message(
                    keys::CLI_DISK_PREVIEW_BYTES,
                    &[("size", &command::technical(&size.to_string()))],
                )
            },
        );
        let item = command::message(
            keys::CLI_DISK_PREVIEW_ITEM,
            &[
                ("name", &command::technical(&name)),
                ("estimate", &estimate),
            ],
        );
        let _ = writeln!(text, "  {item}");
        candidates.push(json!({
            "category": category_key(category),
            "id": row.id,
            "name": row.name,
            "size_bytes": row.size_bytes,
        }));
    }
    let categories: Vec<_> = preview
        .unavailable_categories
        .iter()
        .filter_map(|value| DiskCategory::try_from(*value).ok())
        .map(|category| {
            let row = command::message(
                keys::CLI_DISK_PREVIEW_MISSING_INVENTORY,
                &[("category", category_name(category))],
            );
            let _ = writeln!(text, "{row}");
            category_key(category)
        })
        .collect();
    let estimate = preview.estimated_reclaimable_bytes.map_or_else(
        || unavailable.to_owned(),
        |size| command::technical(&size.to_string()),
    );
    let total = command::message(keys::CLI_DISK_PREVIEW_TOTAL, &[("estimate", &estimate)]);
    let _ = writeln!(text, "{total}");
    Outcome::Data {
        text,
        document: json!({
            "schema_version": 1,
            "kind": "disk_cleanup_preview",
            "apply_supported": false,
            "candidates": candidates,
            "unavailable_categories": categories,
            "estimated_reclaimable_bytes": preview.estimated_reclaimable_bytes,
        }),
    }
}

async fn connect() -> Result<MarsaClient, Failure> {
    DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)
}

fn connection_failure(error: ClientError) -> Failure {
    match error {
        ClientError::Home => Failure::Home,
        ClientError::Root => Failure::Root,
        ClientError::IncompatibleVersion { .. } | ClientError::MissingVersionService => {
            Failure::Incompatible
        }
        _ => Failure::Connection,
    }
}

fn report(usage: &DiskUsageResponse) -> (Value, String) {
    let row = |key, bytes: u64| {
        command::message(key, &[("bytes", &command::technical(&bytes.to_string()))])
    };
    let mut text = format!(
        "{}\n{}\n",
        row(keys::CLI_DISK_HOST_ALLOCATION, usage.host_allocated_bytes),
        row(keys::CLI_DISK_HOST_CAPACITY, usage.host_capacity_bytes),
    );
    let guest = if usage.guest_usage_available {
        let _ = writeln!(
            text,
            "{}\n{}",
            row(keys::CLI_DISK_GUEST_USED, usage.guest_used_bytes),
            row(keys::CLI_DISK_GUEST_CAPACITY, usage.guest_capacity_bytes),
        );
        Some(json!({
            "used_bytes": usage.guest_used_bytes,
            "capacity_bytes": usage.guest_capacity_bytes,
        }))
    } else {
        let unavailable = command::label(keys::CLI_DISK_GUEST_UNAVAILABLE);
        let _ = writeln!(text, "{unavailable}");
        None
    };
    let categories: Vec<Value> = usage
        .categories
        .iter()
        .map(|item| {
            let category = item.category();
            let message = command::message(
                keys::CLI_DISK_CATEGORY_USAGE,
                &[
                    ("category", category_name(category)),
                    ("used", &command::technical(&item.used_bytes.to_string())),
                    (
                        "reclaimable",
                        &command::technical(&item.reclaimable_bytes.to_string()),
                    ),
                ],
            );
            let _ = writeln!(text, "{message}");
            json!({
                "category": category_key(category),
                "used_bytes": item.used_bytes,
                "reclaimable_bytes": item.reclaimable_bytes,
            })
        })
        .collect();
    (
        json!({
            "schema_version": 1,
            "kind": "disk_usage",
            "host": {
                "allocated_bytes": usage.host_allocated_bytes,
                "capacity_bytes": usage.host_capacity_bytes,
            },
            "guest": guest,
            "categories": categories,
        }),
        text,
    )
}

fn category_key(category: DiskCategory) -> String {
    category
        .as_str_name()
        .trim_start_matches("DISK_CATEGORY_")
        .to_ascii_lowercase()
}

fn category_name(category: DiskCategory) -> &'static str {
    match category {
        DiskCategory::Containers => command::label(keys::CLI_DISK_CONTAINERS),
        DiskCategory::Images => command::label(keys::CLI_DISK_IMAGES),
        DiskCategory::Volumes => command::label(keys::CLI_DISK_VOLUMES),
        DiskCategory::Networks => command::label(keys::CLI_DISK_NETWORKS),
        DiskCategory::BuildCache => command::label(keys::CLI_DISK_BUILD_CACHE),
        DiskCategory::Unspecified => command::label(keys::CLI_DISK_OTHER),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsa_client::api::{DiskCategoryUsage, PruneCandidate};

    #[test]
    fn cleanup_json_preserves_unknown_estimates_without_offering_apply() {
        let preview = PrunePreviewResponse {
            candidates: vec![PruneCandidate {
                category: DiskCategory::Containers as i32,
                id: "a".repeat(64),
                name: "stopped".to_owned(),
                size_bytes: None,
            }],
            estimated_reclaimable_bytes: None,
            unavailable_categories: vec![DiskCategory::BuildCache as i32],
        };
        let Outcome::Data { document, text } = cleanup_report(&preview) else {
            panic!("expected a read-only result");
        };
        assert_eq!(document["kind"], "disk_cleanup_preview");
        assert_eq!(document["apply_supported"], false);
        assert!(document["estimated_reclaimable_bytes"].is_null());
        assert!(document["candidates"][0]["size_bytes"].is_null());
        assert_eq!(document["unavailable_categories"], json!(["build_cache"]));
        assert!(text.contains("Read-only cleanup preview"));
        assert!(text.contains("Build cache  inventory unavailable"));
    }

    #[test]
    fn reports_sparse_allocation_and_leaves_missing_guest_usage_null() {
        let (document, text) = report(&DiskUsageResponse {
            host_capacity_bytes: 1 << 30,
            host_allocated_bytes: 4096,
            ..Default::default()
        });
        assert_eq!(document["kind"], "disk_usage");
        assert_eq!(document["host"]["capacity_bytes"], 1 << 30);
        assert_eq!(document["host"]["allocated_bytes"], 4096);
        assert_eq!(document["guest"], Value::Null);
        assert!(text.contains("Host allocation (bytes) 4096"));
        assert!(text.contains("Guest filesystem usage is unavailable"));
    }

    #[test]
    fn preserves_a_zero_use_guest_sample_and_category_bytes() {
        let (document, text) = report(&DiskUsageResponse {
            guest_usage_available: true,
            guest_capacity_bytes: 1024,
            categories: vec![DiskCategoryUsage {
                category: DiskCategory::Images.into(),
                used_bytes: 4,
                reclaimable_bytes: 2,
            }],
            ..Default::default()
        });
        assert_eq!(
            document["guest"],
            json!({"used_bytes": 0, "capacity_bytes": 1024})
        );
        assert_eq!(
            document["categories"],
            json!([{"category": "images", "used_bytes": 4, "reclaimable_bytes": 2}])
        );
        assert!(text.contains("Guest filesystem used (bytes)"));
        assert!(text.contains("Images"));
    }
}
