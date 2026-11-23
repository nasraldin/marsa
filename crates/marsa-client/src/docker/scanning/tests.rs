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
