//! Verify the signed target's Marsa schema independently of the TUF client.

use marsa_core::{
    settings::UpdateChannel,
    update_manifest::{ManifestError, ReleaseMetadata, Version},
};
use serde_json::{Value, json};

const TARGET: &str = "releases/marsa-1.4.0-darwin-arm64.zip";

fn metadata() -> Value {
    json!({
        "schema_version": 1,
        "version": "1.4.0",
        "minimum_macos": "27.0.0",
        "channels": ["beta", "stable"],
        "security_recommended": true,
        "notes": {"en-US": "Recovery update", "ar": "تحديث الاستعادة"}
    })
}

#[test]
fn versions_compare_numerically_and_reject_ambiguous_names() {
    assert!("1.10.0".parse::<Version>().unwrap() > "1.9.99".parse::<Version>().unwrap());
    for invalid in [
        "1.2",
        "1.2.3.4",
        "1.02.3",
        "+1.2.3",
        "1.2.3-beta",
        "4294967296.0.0",
    ] {
        assert!(invalid.parse::<Version>().is_err(), "{invalid}");
    }
}

#[test]
fn promotion_keeps_the_archive_name_and_exposes_only_signed_channels() {
    let mut fields = metadata();
    fields["channels"] = json!(["beta"]);
    let beta = ReleaseMetadata::from_verified_target(TARGET, fields).unwrap();
    assert!(beta.includes_channel(UpdateChannel::Beta));
    assert!(!beta.includes_channel(UpdateChannel::Stable));
    let stable = ReleaseMetadata::from_verified_target(TARGET, metadata()).unwrap();
    assert!(stable.includes_channel(UpdateChannel::Stable));
    assert_eq!(stable.target_name(), beta.target_name());
    assert_eq!(stable.version(), beta.version());
    assert_eq!(stable.minimum_macos().to_string(), "27.0.0");
    assert!(stable.security_recommended());
    assert_eq!(stable.notes().arabic, "تحديث الاستعادة");
}

#[test]
fn malformed_custom_fields_and_unexpected_paths_are_refused() {
    for (field, value) in [
        ("schema_version", json!(2)),
        ("minimum_macos", json!("26.0.0")),
        ("channels", json!(["stable", "stable"])),
        ("channels", json!([])),
        ("notes", json!({"en-US": "missing Arabic"})),
        ("notes", json!({"en-US": "x".repeat(8193), "ar": "ar"})),
        ("url", json!("https://example.invalid")),
    ] {
        let mut fields = metadata();
        fields[field] = value;
        assert!(
            ReleaseMetadata::from_verified_target(TARGET, fields).is_err(),
            "{field}"
        );
    }
    assert!(matches!(
        ReleaseMetadata::from_verified_target(
            "releases/../marsa-1.4.0-darwin-arm64.zip",
            metadata()
        ),
        Err(ManifestError::TargetName)
    ));
}
