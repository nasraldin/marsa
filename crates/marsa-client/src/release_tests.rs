//! Tests for the installed release's independently available resources.

use std::{fs, path::Path};

use crate::{
    l10n::Locale,
    release::{AboutAssets, Evidence},
};

fn write(root: &Path, name: &str, content: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().expect("test asset parent")).expect("asset directory");
    fs::write(path, content).expect("write test asset");
}

#[test]
fn loads_installed_notes_docker_tool_versions_and_evidence() {
    let resources = tempfile::tempdir().expect("temporary Resources directory");
    let root = resources.path();
    write(
        root,
        "release-notes.json",
        r#"{"version":"0.1.0","en-US":"Release notes","ar":"ملاحظات الإصدار"}"#,
    );
    write(
        root,
        "licenses/upstream-sbom-input.json",
        r#"{"components":[{"name":"docker","version":"30.3.0"},{"name":"docker-compose","version":"5.0.0"}]}"#,
    );
    write(root, "licenses/marsa-sbom.cdx.json", "{}");

    let assets = AboutAssets::read(root, "0.1.0");
    assert_eq!(assets.notes(Locale::EnglishUs), Some("Release notes"));
    assert_eq!(assets.notes(Locale::Arabic), Some("ملاحظات الإصدار"));
    assert_eq!(
        assets
            .host_tools()
            .iter()
            .map(|component| (component.name.as_str(), component.version.as_str()))
            .collect::<Vec<_>>(),
        [("docker", "30.3.0"), ("docker-compose", "5.0.0")],
    );
    assert_eq!(
        assets.evidence(Evidence::AppSbom),
        Some(root.join("licenses/marsa-sbom.cdx.json").as_path())
    );
    assert_eq!(assets.evidence(Evidence::GuestSbom), None);
}

#[test]
fn stale_or_incomplete_notes_and_invalid_components_do_not_supply_details() {
    let resources = tempfile::tempdir().expect("temporary Resources directory");
    let root = resources.path();
    write(
        root,
        "release-notes.json",
        r#"{"version":"0.0.1","en-US":"Old release","ar":"إصدار سابق"}"#,
    );
    write(
        root,
        "licenses/upstream-sbom-input.json",
        r#"{"components":[{"name":"docker","version":""}]}"#,
    );
    let assets = AboutAssets::read(root, "0.1.0");
    assert_eq!(assets.notes(Locale::EnglishUs), None);
    assert!(assets.host_tools().is_empty());

    write(
        root,
        "release-notes.json",
        r#"{"version":"0.1.0","en-US":"Release notes"}"#,
    );
    assert_eq!(
        AboutAssets::read(root, "0.1.0").notes(Locale::EnglishUs),
        None
    );
}
