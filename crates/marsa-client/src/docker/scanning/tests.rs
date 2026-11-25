use super::*;

#[test]
fn normalized_grype_findings_exclude_unrelated_report_data() {
    let input = br#"{"matches":[{"vulnerability":{"id":"CVE-2026-1234","severity":"High","fix":{"versions":["1.2.0"]}},"artifact":{"name":"openssl","version":"1.1.0"}}],"ignoredMatches":[{"other":"unused"}]}"#;
    let report = parse_report(Scanner::Grype, input).unwrap();
    assert_eq!(report.total, 1);
    assert_eq!(report.findings[0].severity, Severity::High);
    assert_eq!(report.findings[0].fix, "1.2.0");
}

#[test]
fn trivy_requires_a_container_report_even_when_there_are_no_results() {
    assert!(parse_report(Scanner::Trivy, br#"{}"#).is_err());
    let input = br#"{"SchemaVersion":2,"ArtifactType":"container_image"}"#;
    assert_eq!(parse_report(Scanner::Trivy, input).unwrap().total, 0);
    let input = br#"{"SchemaVersion":2,"ArtifactType":"container_image","Results":[{"Vulnerabilities":[{"VulnerabilityID":"CVE-2026-1234","Severity":"CRITICAL","PkgName":"example","InstalledVersion":"1"}]}]}"#;
    let report = parse_report(Scanner::Trivy, input).unwrap();
    assert_eq!(report.findings[0].severity, Severity::Critical);
}

#[tokio::test]
async fn scanner_opt_in_is_isolated_by_environment() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let a = ScanSettingsStore {
        directory: directory.path().to_owned(),
        key: "a".into(),
    };
    let b = ScanSettingsStore {
        directory: directory.path().to_owned(),
        key: "b".into(),
    };
    let preference = ScanSettings {
        enabled: true,
        choice: ScannerChoice::Both,
        ..ScanSettings::default()
    };
    a.save(preference).await.unwrap();
    assert_eq!(a.load().await.unwrap(), preference);
    assert_eq!(b.load().await.unwrap(), ScanSettings::default());
    a.save(ScanSettings {
        enabled: false,
        ..preference
    })
    .await
    .unwrap();
    assert!(!a.load().await.unwrap().enabled);
}

#[tokio::test]
async fn retention_accepts_previous_settings_and_expires_only_its_engine_report() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let first = ScanSettingsStore {
        directory: directory.path().to_owned(),
        key: "first".into(),
    };
    let second = ScanSettingsStore {
        directory: directory.path().to_owned(),
        key: "second".into(),
    };
    platform::replace_docker_state(&first.path(), br#"{"enabled":true,"choice":"grype"}"#).unwrap();
    assert_eq!(
        first.load().await.unwrap().retention,
        ScanRetention::SessionOnly
    );

    for store in [&first, &second] {
        let settings = ScanSettings {
            retention: ScanRetention::OneDay,
            ..ScanSettings::default()
        };
        store.save(settings).await.unwrap();
        store
            .record_at(
                &ImageId::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
                &[(
                    Scanner::Grype,
                    Ok(ScanReport {
                        total: 0,
                        findings: vec![],
                    }),
                )],
                100,
            )
            .await
            .unwrap();
    }

    let renewed = second.history_at(101).await.unwrap();
    assert_eq!(renewed.outcome, RetentionOutcome::Retained);
    assert_eq!(renewed.report.unwrap().reports()[0].0, Scanner::Grype);
    let expired = first.history_at(100 + 86_400).await.unwrap();
    assert_eq!(expired.outcome, RetentionOutcome::Expired);
    assert!(expired.report.is_none());
    assert!(!first.report_path().exists());
    assert!(second.report_path().exists());

    assert_eq!(
        second.save(ScanSettings::default()).await.unwrap(),
        Some(RetentionOutcome::Discarded)
    );
    assert!(!second.report_path().exists());
}

#[test]
fn cache_cleanup_does_not_follow_links_or_remove_its_lock() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let owner = tempfile::tempdir().unwrap();
    let cache = owner.path().join("cache");
    let lock = platform::lock_docker_state(&cache, "scan.lock").unwrap();
    let unrelated = owner.path().join("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep"), b"keep").unwrap();
    std::os::unix::fs::symlink(&unrelated, cache.join("link")).unwrap();
    std::fs::create_dir(cache.join("db")).unwrap();
    std::fs::write(cache.join("db/reports"), b"cache").unwrap();
    drop(lock);

    assert!(clear_cache(&cache).unwrap());
    assert!(!clear_cache(&cache).unwrap());
    assert!(cache.join("scan.lock").is_file());
    assert!(unrelated.join("keep").is_file());
}
