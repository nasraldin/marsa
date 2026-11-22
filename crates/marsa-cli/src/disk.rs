//! Report the daemon's sparse image and guest filesystem observations.

use std::fmt::Write as _;

use marsa_client::{
    ClientError, DaemonEndpoint,
    api::{DiskCategory, DiskUsageRequest, DiskUsageResponse},
};
use serde_json::{Value, json};

use crate::{Failure, Outcome, command::Language};

pub(super) async fn usage(language: Language) -> Result<Outcome, Failure> {
    let client = DaemonEndpoint::discover()
        .map_err(connection_failure)?
        .connect()
        .await
        .map_err(connection_failure)?;
    let usage = client
        .disk()
        .usage(DiskUsageRequest {})
        .await
        .map_err(|_| Failure::Operation)?
        .into_inner();
    Ok(report(&usage, language))
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

fn report(usage: &DiskUsageResponse, language: Language) -> Outcome {
    let allocated = language.text(
        "Host allocation (bytes)",
        "المساحة المخصّصة على المضيف (بايت)",
    );
    let capacity = language.text(
        "Host image capacity (bytes)",
        "سعة صورة القرص على المضيف (بايت)",
    );
    let mut text = format!(
        "{allocated} {}\n{capacity} {}\n",
        usage.host_allocated_bytes, usage.host_capacity_bytes,
    );
    let guest = if usage.guest_usage_available {
        let used = language.text(
            "Guest filesystem used (bytes)",
            "المساحة المستخدمة في نظام ملفات الضيف (بايت)",
        );
        let capacity = language.text(
            "Guest filesystem capacity (bytes)",
            "سعة نظام ملفات الضيف (بايت)",
        );
        let _ = writeln!(
            text,
            "{used} {}\n{capacity} {}",
            usage.guest_used_bytes, usage.guest_capacity_bytes
        );
        Some(json!({
            "used_bytes": usage.guest_used_bytes,
            "capacity_bytes": usage.guest_capacity_bytes,
        }))
    } else {
        let unavailable = language.text(
            "Guest filesystem usage is unavailable",
            "استخدام نظام ملفات الضيف غير متاح",
        );
        let _ = writeln!(text, "{unavailable}");
        None
    };
    let categories: Vec<Value> = usage
        .categories
        .iter()
        .map(|item| {
            let category = item.category();
            let label = category_name(category, language);
            let used = language.text("used bytes", "بايت مستخدم");
            let reclaimable = language.text("reclaimable bytes", "بايت قابل للاسترداد");
            let _ = writeln!(text, "{label} {} {used}, {} {reclaimable}", item.used_bytes, item.reclaimable_bytes);
            json!({
                "category": category.as_str_name().trim_start_matches("DISK_CATEGORY_").to_ascii_lowercase(),
                "used_bytes": item.used_bytes,
                "reclaimable_bytes": item.reclaimable_bytes,
            })
        })
        .collect();
    Outcome::Data {
        text,
        document: json!({
            "schema_version": 1,
            "kind": "disk_usage",
            "host": {
                "allocated_bytes": usage.host_allocated_bytes,
                "capacity_bytes": usage.host_capacity_bytes,
            },
            "guest": guest,
            "categories": categories,
        }),
    }
}

fn category_name(category: DiskCategory, language: Language) -> &'static str {
    match category {
        DiskCategory::Containers => language.text("Containers", "الحاويات"),
        DiskCategory::Images => language.text("Images", "الصور"),
        DiskCategory::Volumes => language.text("Volumes", "وحدات التخزين"),
        DiskCategory::Networks => language.text("Networks", "الشبكات"),
        DiskCategory::BuildCache => language.text("Build cache", "ذاكرة البناء المؤقتة"),
        DiskCategory::Unspecified => language.text("Other disk usage", "استخدامات القرص الأخرى"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsa_client::api::DiskCategoryUsage;

    #[test]
    fn reports_sparse_allocation_and_leaves_missing_guest_usage_null() {
        let Outcome::Data { document, text } = report(
            &DiskUsageResponse {
                host_capacity_bytes: 1 << 30,
                host_allocated_bytes: 4096,
                ..Default::default()
            },
            Language::English,
        ) else {
            panic!("expected usage data")
        };
        assert_eq!(document["kind"], "disk_usage");
        assert_eq!(document["host"]["capacity_bytes"], 1 << 30);
        assert_eq!(document["host"]["allocated_bytes"], 4096);
        assert_eq!(document["guest"], Value::Null);
        assert!(text.contains("Host allocation (bytes) 4096"));
        assert!(text.contains("Guest filesystem usage is unavailable"));
    }

    #[test]
    fn preserves_a_zero_use_guest_sample_and_category_bytes() {
        let Outcome::Data { document, text } = report(
            &DiskUsageResponse {
                guest_usage_available: true,
                guest_capacity_bytes: 1024,
                categories: vec![DiskCategoryUsage {
                    category: DiskCategory::Images.into(),
                    used_bytes: 4,
                    reclaimable_bytes: 2,
                }],
                ..Default::default()
            },
            Language::Arabic,
        ) else {
            panic!("expected usage data")
        };
        assert_eq!(
            document["guest"],
            json!({"used_bytes": 0, "capacity_bytes": 1024})
        );
        assert_eq!(
            document["categories"],
            json!([{"category": "images", "used_bytes": 4, "reclaimable_bytes": 2}])
        );
        assert!(text.contains("المساحة المستخدمة في نظام ملفات الضيف"));
        assert!(text.contains("الصور"));
    }
}
