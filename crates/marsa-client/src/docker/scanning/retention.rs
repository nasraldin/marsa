//! Retain only the latest normalized successful scan for each engine.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    Deserialize, DockerClient, File, ImageId, ScanError, ScanReport, ScanSettingsStore, Scanner,
    ScannerResult, Serialize, io, platform,
};

const SAVED_REPORT_LIMIT: usize = 1024 * 1024;

/// Retention for normalized findings after closing the environment editor.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanRetention {
    /// Keep findings in the current editor only.
    #[default]
    SessionOnly,
    /// Retain the latest successful results for one day.
    OneDay,
    /// Retain the latest successful results for seven days.
    SevenDays,
}

impl ScanRetention {
    fn seconds(self) -> Option<u64> {
        match self {
            Self::SessionOnly => None,
            Self::OneDay => Some(86_400),
            Self::SevenDays => Some(7 * 86_400),
        }
    }
}

/// What happened to the selected engine's saved report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionOutcome {
    /// A report is available from private storage.
    Retained,
    /// Findings stay in the current editor only.
    SessionOnly,
    /// The previous report was deleted when retention was turned off.
    Discarded,
    /// The previous report was removed because it exceeded the saved lifetime.
    Expired,
    /// The engine has no saved report.
    Empty,
    /// This scan did not replace the previous report.
    Unchanged,
}

/// A saved scan associated with its original immutable image.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedScan {
    image: String,
    scanned_at: u64,
    reports: Vec<(Scanner, ScanReport)>,
}

impl RetainedScan {
    /// Full immutable image ID from the completed scan.
    pub fn image(&self) -> &str {
        &self.image
    }

    /// Successfully normalized scanner findings.
    pub fn reports(&self) -> &[(Scanner, ScanReport)] {
        &self.reports
    }
}

/// Latest saved scan after applying the current environment retention policy.
pub struct ScanHistory {
    /// Saved report, when still eligible to reopen.
    pub report: Option<RetainedScan>,
    /// Whether a previously saved report is available, empty or removed.
    pub outcome: RetentionOutcome,
}

impl DockerClient {
    /// Read the latest retained results and remove the report if it expired.
    pub async fn scan_history(&self) -> Result<ScanHistory, ScanError> {
        ScanSettingsStore::for_client(self)?
            .history_at(timestamp()?)
            .await
    }
}

pub(super) fn timestamp() -> Result<u64, ScanError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| ScanError::State(io::Error::other(error)))
}

impl ScanSettingsStore {
    pub(super) async fn history_at(&self, now: u64) -> Result<ScanHistory, ScanError> {
        let lock = Arc::new(self.lock().await?);
        self.history_locked(now, self.load().await?.retention, &lock)
            .await
    }

    pub(super) async fn record_at(
        &self,
        image: &ImageId,
        results: &[ScannerResult],
        now: u64,
    ) -> Result<RetentionOutcome, ScanError> {
        let lock = Arc::new(self.lock().await?);
        let retention = self.load().await?.retention;
        let history = self.history_locked(now, retention, &lock).await?;

        if retention == ScanRetention::SessionOnly {
            return Ok(if history.outcome == RetentionOutcome::Discarded {
                history.outcome
            } else {
                RetentionOutcome::SessionOnly
            });
        }

        let reports: Vec<_> = results
            .iter()
            .filter_map(|(scanner, result)| Some((*scanner, result.as_ref().ok()?.clone())))
            .collect();
        if reports.is_empty() {
            return Ok(if history.outcome == RetentionOutcome::Expired {
                history.outcome
            } else {
                RetentionOutcome::Unchanged
            });
        }

        let path = self.report_path();
        let report = RetainedScan {
            image: image.as_str().to_owned(),
            scanned_at: now,
            reports,
        };
        let bytes = serde_json::to_vec(&report)
            .map_err(|error| ScanError::State(io::Error::other(error)))?;
        if bytes.len() > SAVED_REPORT_LIMIT {
            return Err(ScanError::Report);
        }
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            platform::replace_docker_state(&path, &bytes)
        })
        .await
        .map_err(|error| ScanError::State(io::Error::other(error)))?
        .map_err(ScanError::State)?;
        Ok(RetentionOutcome::Retained)
    }

    async fn history_locked(
        &self,
        now: u64,
        retention: ScanRetention,
        lock: &Arc<File>,
    ) -> Result<ScanHistory, ScanError> {
        let path = self.report_path();
        let bytes = match platform::read_user_docker_file(&path, SAVED_REPORT_LIMIT, true).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ScanHistory {
                    report: None,
                    outcome: RetentionOutcome::Empty,
                });
            }
            Err(error) => return Err(ScanError::State(error)),
        };

        let outcome = if let Some(seconds) = retention.seconds() {
            let report: RetainedScan =
                serde_json::from_slice(&bytes).map_err(|_| ScanError::Report)?;
            ImageId::parse(report.image.as_str()).map_err(|_| ScanError::Report)?;
            if now
                .checked_sub(report.scanned_at)
                .is_some_and(|age| age < seconds)
            {
                return Ok(ScanHistory {
                    report: Some(report),
                    outcome: RetentionOutcome::Retained,
                });
            }
            RetentionOutcome::Expired
        } else {
            RetentionOutcome::Discarded
        };
        let lock = lock.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            std::fs::remove_file(path)
        })
        .await
        .map_err(|error| ScanError::State(io::Error::other(error)))?
        .map_err(ScanError::State)?;
        Ok(ScanHistory {
            report: None,
            outcome,
        })
    }

    pub(super) fn report_path(&self) -> std::path::PathBuf {
        self.directory
            .join(format!("desktop-scan-report-{}.json", self.key))
    }
}
