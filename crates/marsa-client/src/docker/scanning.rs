//! Owner-triggered defensive scans of one engine-scoped immutable image.
//!
//! The scanners receive a private Docker archive and a clean environment.
//! The engine socket and registry credentials remain with the Docker client.

use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use tokio::{fs, io::AsyncReadExt as _, process::Command, time::timeout};

use crate::{docker_socket_path, platform};

use super::{DockerClient, DockerError, DockerScope, ImageId};

const IMAGE_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const REPORT_LIMIT: u64 = 32 * 1024 * 1024;
const MAX_FINDINGS: usize = 100;
const SCANNER_DEADLINE: Duration = Duration::from_secs(600);

/// Scanner selection saved for an environment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScannerChoice {
    /// Scan with Anchore Grype.
    #[default]
    Grype,
    /// Scan with Aqua Trivy.
    Trivy,
    /// Run both scanners against the same image archive.
    Both,
}

impl ScannerChoice {
    fn scanners(self) -> &'static [Scanner] {
        match self {
            Self::Grype => &[Scanner::Grype],
            Self::Trivy => &[Scanner::Trivy],
            Self::Both => &[Scanner::Grype, Scanner::Trivy],
        }
    }
}

/// Whether the owner opted in and which scanner to run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScanSettings {
    /// Enable manually requested scans in this environment.
    pub enabled: bool,
    /// Tool selection, retained when scanning is disabled.
    pub choice: ScannerChoice,
}

/// Tool responsible for an individual result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scanner {
    /// Anchore Grype.
    Grype,
    /// Aqua Trivy.
    Trivy,
}

impl Scanner {
    /// Tool's display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Grype => "Grype",
            Self::Trivy => "Trivy",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Grype => "grype",
            Self::Trivy => "trivy",
        }
    }
}

/// The severity assigned by a vulnerability scanner.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Scanner could not assign severity.
    Unknown,
    /// Negligible risk.
    Negligible,
    /// Low risk.
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    High,
    /// Critical risk.
    Critical,
}

/// Display fields selected from the scanner response.
#[derive(Debug)]
pub struct Finding {
    /// Identifier reported by the scanner.
    pub id: String,
    /// Package associated with the advisory.
    pub package: String,
    /// Version installed in the image.
    pub installed: String,
    /// Fixed versions when supplied.
    pub fix: String,
    /// Severity reported by the scanner.
    pub severity: Severity,
}

/// One scanner's result for the selected image.
#[derive(Debug)]
pub struct ScanReport {
    /// Total advisories in the result, including any omitted from the preview.
    pub total: usize,
    /// Highest severity advisories, bounded for display.
    pub findings: Vec<Finding>,
}

/// The scanner and outcome for a requested image.
pub type ScannerResult = (Scanner, Result<ScanReport, ScanError>);

/// Safe-to-display categories for image scanning failures.
#[derive(Debug, Error)]
pub enum ScanError {
    /// This operation requires the desktop owner account.
    #[error("image scanning requires a non-root user")]
    Root,
    /// The selected engine is unavailable.
    #[error("the selected engine is unavailable")]
    Endpoint,
    /// The owner has not authorized scans in this environment.
    #[error("scanning is disabled for this environment")]
    Disabled,
    /// Settings or private workspace are unavailable.
    #[error("could not access the image scanner's private files")]
    State(#[source] io::Error),
    /// A required executable is unavailable at its trusted installation locations.
    #[error("could not find an installed scanner")]
    Missing,
    /// The image could not be exported from the selected engine.
    #[error("could not read an image archive from the selected engine")]
    Export(#[source] DockerError),
    /// The scanner process failed, including unavailable vulnerability databases.
    #[error("the scanner did not complete")]
    Process,
    /// The installed scanner produced an unexpected or oversized report.
    #[error("the scanner returned an invalid or oversized report")]
    Report,
    /// The scanner exceeded its time limit.
    #[error("the scanner timed out")]
    Timeout,
}

struct ScanSettingsStore {
    directory: PathBuf,
    key: String,
}

impl ScanSettingsStore {
    fn for_client(client: &DockerClient) -> Result<Self, ScanError> {
        check_user()?;
        let socket =
            docker_socket_path().map_err(|error| ScanError::State(io::Error::other(error)))?;
        let directory = socket
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| ScanError::State(io::ErrorKind::InvalidInput.into()))?;
        let mut hash = Sha256::new();
        match &client.scope {
            DockerScope::Marsa(socket) => {
                hash.update(b"local\0");
                hash.update(socket.as_os_str().as_encoded_bytes());
            }
            DockerScope::External { endpoint, .. } => {
                let address = endpoint.address();
                if address.is_empty() || address.len() > 4096 {
                    return Err(ScanError::Endpoint);
                }
                hash.update(b"external\0");
                hash.update(endpoint.scanning_context().unwrap_or("").as_bytes());
                hash.update(b"\0");
                hash.update(address.as_bytes());
            }
        }
        Ok(Self {
            directory: directory.to_owned(),
            key: URL_SAFE_NO_PAD.encode(hash.finalize()),
        })
    }

    async fn load(&self) -> Result<ScanSettings, ScanError> {
        check_user()?;
        let read = async {
            platform::check_user_docker_directory(&self.directory).await?;
            platform::read_user_docker_file(&self.path(), 4096, true).await
        };
        match read.await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| ScanError::State(io::Error::other(error))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ScanSettings::default()),
            Err(error) => Err(ScanError::State(error)),
        }
    }

    async fn save(&self, settings: ScanSettings) -> Result<(), ScanError> {
        check_user()?;
        let directory = self.directory.clone();
        let path = self.path();
        let data = serde_json::to_vec(&settings)
            .map_err(|error| ScanError::State(io::Error::other(error)))?;
        tokio::task::spawn_blocking(move || {
            let _lock = platform::lock_docker_state(&directory, "desktop-scanners.lock")?;
            platform::replace_docker_state(&path, &data)
        })
        .await
        .map_err(|error| ScanError::State(io::Error::other(error)))?
        .map_err(ScanError::State)
    }

    fn path(&self) -> PathBuf {
        self.directory
            .join(format!("desktop-scanner-{}.json", self.key))
    }
}

impl DockerClient {
    /// Load the owner's scanner preferences for this exact endpoint.
    pub async fn scan_settings(&self) -> Result<ScanSettings, ScanError> {
        ScanSettingsStore::for_client(self)?.load().await
    }

    /// Atomically replace the owner's scanner preferences for this endpoint.
    pub async fn save_scan_settings(&self, settings: ScanSettings) -> Result<(), ScanError> {
        ScanSettingsStore::for_client(self)?.save(settings).await
    }

    /// Scan this engine's selected immutable image according to the saved opt-in.
    ///
    /// A missing tool in Both mode yields an individual error, while an installed
    /// tool can still return its findings.
    pub async fn scan_image(&self, image: &ImageId) -> Result<Vec<ScannerResult>, ScanError> {
        let store = ScanSettingsStore::for_client(self)?;
        let settings = store.load().await?;
        if !settings.enabled {
            return Err(ScanError::Disabled);
        }
        let mut tools = Vec::new();
        for scanner in settings.choice.scanners() {
            tools.push((
                *scanner,
                platform::scanner_executable(scanner.executable()).await,
            ));
        }
        if tools.iter().all(|(_, binary)| binary.is_none()) {
            return Ok(tools
                .into_iter()
                .map(|(scanner, _)| (scanner, Err(ScanError::Missing)))
                .collect());
        }

        let directory = store.directory;
        let private = tokio::task::spawn_blocking({
            let directory = directory.clone();
            move || {
                tempfile::Builder::new()
                    .prefix(".marsa-scan-")
                    .tempdir_in(directory)
            }
        })
        .await
        .map_err(|error| ScanError::State(io::Error::other(error)))?
        .map_err(ScanError::State)?;
        let archive = private.path().join("image.tar");
        self.export_image_with_limit(image, &archive, IMAGE_LIMIT)
            .await
            .map_err(ScanError::Export)?;

        let config = private.path().join("scanner.yaml");
        fs::write(&config, "{}\n").await.map_err(ScanError::State)?;
        let mut reports = Vec::with_capacity(tools.len());
        for (scanner, binary) in tools {
            let report = match binary {
                Some(binary) => {
                    let cache = directory.join(format!("desktop-scanner-{}", scanner.executable()));
                    run(scanner, &binary, private.path(), &config, &archive, &cache).await
                }
                None => Err(ScanError::Missing),
            };
            reports.push((scanner, report));
        }
        Ok(reports)
    }
}

fn check_user() -> Result<(), ScanError> {
    if nix::unistd::geteuid().is_root() {
        return Err(ScanError::Root);
    }
    Ok(())
}

async fn run(
    scanner: Scanner,
    program: &Path,
    workspace: &Path,
    config: &Path,
    archive: &Path,
    cache: &Path,
) -> Result<ScanReport, ScanError> {
    let _lock = tokio::task::spawn_blocking({
        let cache = cache.to_owned();
        move || platform::lock_docker_state(&cache, "scan.lock")
    })
    .await
    .map_err(|error| ScanError::State(io::Error::other(error)))?
    .map_err(ScanError::State)?;

    let mut command = Command::new(program);
    command
        .env_clear()
        .env("HOME", workspace)
        .env("XDG_CONFIG_HOME", workspace)
        .env("XDG_CACHE_HOME", workspace)
        .env("DOCKER_CONFIG", workspace)
        .env("TMPDIR", workspace)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match scanner {
        Scanner::Grype => {
            let mut source = std::ffi::OsString::from("docker-archive:");
            source.push(archive);
            command
                .env("GRYPE_DB_CACHE_DIR", cache)
                .env("GRYPE_CHECK_FOR_APP_UPDATE", "false")
                .env("GRYPE_EXTERNAL_SOURCES_ENABLE", "false")
                .env("SYFT_CHECK_FOR_APP_UPDATE", "false")
                .arg("--config")
                .arg(config)
                .args(["--output", "json"])
                .arg(source);
        }
        Scanner::Trivy => {
            command
                .arg("--config")
                .arg(config)
                .arg("--cache-dir")
                .arg(cache)
                .args([
                    "image",
                    "--cache-backend",
                    "memory",
                    "--disable-telemetry",
                    "--skip-version-check",
                    "--offline-scan",
                    "--scanners",
                    "vuln",
                    "--format",
                    "json",
                    "--no-progress",
                    "--db-repository",
                    "ghcr.io/aquasecurity/trivy-db:2,mirror.gcr.io/aquasec/trivy-db:2",
                    "--input",
                ])
                .arg(archive);
        }
    }

    let bytes = timeout(SCANNER_DEADLINE, async {
        let mut child = command.spawn().map_err(|_| ScanError::Process)?;
        let output = child.stdout.take().ok_or(ScanError::Process)?;
        let mut bytes = Vec::new();
        output
            .take(REPORT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ScanError::Process)?;
        if bytes.len() as u64 > REPORT_LIMIT {
            return Err(ScanError::Report);
        }
        if !child
            .wait()
            .await
            .map_err(|_| ScanError::Process)?
            .success()
        {
            return Err(ScanError::Process);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| ScanError::Timeout)??;

    tokio::task::spawn_blocking(move || parse_report(scanner, &bytes))
        .await
        .map_err(|_| ScanError::Report)?
}

#[derive(Deserialize)]
struct GrypeReport {
    matches: Vec<GrypeMatch>,
}

#[derive(Deserialize)]
struct GrypeMatch {
    artifact: GrypeArtifact,
    vulnerability: GrypeVulnerability,
}

#[derive(Deserialize)]
struct GrypeArtifact {
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct GrypeVulnerability {
    id: String,
    severity: String,
    fix: Option<GrypeFix>,
}

#[derive(Deserialize)]
struct GrypeFix {
    versions: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TrivyReport {
    schema_version: u32,
    artifact_type: String,
    #[serde(default)]
    results: Vec<TrivyResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TrivyResult {
    #[serde(default)]
    vulnerabilities: Vec<TrivyVulnerability>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TrivyVulnerability {
    #[serde(rename = "VulnerabilityID")]
    id: String,
    pkg_name: String,
    installed_version: String,
    severity: String,
    fixed_version: Option<String>,
}

fn parse_report(scanner: Scanner, bytes: &[u8]) -> Result<ScanReport, ScanError> {
    let mut findings: Vec<_> = match scanner {
        Scanner::Grype => serde_json::from_slice::<GrypeReport>(bytes)
            .map_err(|_| ScanError::Report)?
            .matches
            .into_iter()
            .map(|entry| {
                let vuln = entry.vulnerability;
                let fix = vuln.fix.and_then(|fix| fix.versions).unwrap_or_default();
                Finding::new(
                    &vuln.id,
                    &entry.artifact.name,
                    &entry.artifact.version,
                    fix.first().map_or("", String::as_str),
                    &vuln.severity,
                )
            })
            .collect(),
        Scanner::Trivy => {
            let report: TrivyReport =
                serde_json::from_slice(bytes).map_err(|_| ScanError::Report)?;
            if report.schema_version != 2 || report.artifact_type != "container_image" {
                return Err(ScanError::Report);
            }
            report
                .results
                .into_iter()
                .flat_map(|result| result.vulnerabilities)
                .map(|entry| {
                    Finding::new(
                        &entry.id,
                        &entry.pkg_name,
                        &entry.installed_version,
                        entry.fixed_version.as_deref().unwrap_or_default(),
                        &entry.severity,
                    )
                })
                .collect()
        }
    };
    let total = findings.len();
    findings.sort_unstable_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.package.cmp(&b.package))
    });
    findings.truncate(MAX_FINDINGS);
    Ok(ScanReport { total, findings })
}

impl Finding {
    fn new(id: &str, package: &str, installed: &str, fix: &str, severity: &str) -> Self {
        Self {
            id: display_field(id),
            package: display_field(package),
            installed: display_field(installed),
            fix: display_field(fix),
            severity: match severity.to_ascii_lowercase().as_str() {
                "negligible" => Severity::Negligible,
                "low" => Severity::Low,
                "medium" => Severity::Medium,
                "high" => Severity::High,
                "critical" => Severity::Critical,
                _ => Severity::Unknown,
            },
        }
    }
}

fn display_field(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests;
