//! Store only typed decision metadata, with bounded rotation and owner queries.

mod platform;

use std::{
    io::{self, Read, Write},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use marsa_core::paths::MarsaPaths;
use marsa_proto::host::v1::{
    AgentAuditRecord, AgentOperation, AuditApprover, AuditArguments, AuditDecision, AuditResult,
    ListAgentAuditRequest, ListAgentAuditResponse,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FILES: &[&str] = &[
    "agent-audit.jsonl",
    "agent-audit.1.jsonl",
    "agent-audit.2.jsonl",
    "agent-audit.3.jsonl",
];
const FILE_LIMIT: u64 = 512 * 1024;
const RECORD_LIMIT: usize = 1024;
const PAGE_LIMIT: usize = 100;
const DEFAULT_PAGE: usize = 25;

pub(super) struct Store {
    paths: MarsaPaths,
    uid: u32,
    lock: Mutex<()>,
    segment_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    id: String,
    received_at_unix_millis: i64,
    client_ref: Option<String>,
    operation: i32,
    arguments: i32,
    decision: i32,
    approver: i32,
    result: i32,
}

impl Record {
    pub(super) fn new(operation: AgentOperation) -> io::Result<Self> {
        let mut id = [0; 16];
        getrandom::fill(&mut id).map_err(io::Error::other)?;
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        Ok(Self {
            id: hex(&id),
            received_at_unix_millis: i64::try_from(elapsed.as_millis())
                .map_err(io::Error::other)?,
            client_ref: None,
            operation: operation.into(),
            arguments: AuditArguments::Omitted.into(),
            decision: AuditDecision::Refused.into(),
            approver: AuditApprover::None.into(),
            result: AuditResult::Rejected.into(),
        })
    }

    pub(super) fn verified_client(&mut self, client_id: &str) {
        self.client_ref = Some(hex(&Sha256::digest(client_id.as_bytes())));
    }

    pub(super) fn arguments(&mut self, arguments: AuditArguments) {
        self.arguments = arguments.into();
    }

    pub(super) fn complete(&mut self, decision: AuditDecision, result: AuditResult) {
        self.decision = decision.into();
        self.result = result.into();
        self.approver = if matches!(decision, AuditDecision::Approved | AuditDecision::Denied) {
            AuditApprover::LocalOwner
        } else {
            AuditApprover::None
        }
        .into();
    }

    fn valid(&self) -> bool {
        valid_hex(&self.id, 32)
            && self.received_at_unix_millis >= 0
            && self
                .client_ref
                .as_ref()
                .is_none_or(|value| valid_hex(value, 64))
            && AgentOperation::try_from(self.operation).is_ok()
            && AuditArguments::try_from(self.arguments).is_ok()
            && AuditDecision::try_from(self.decision).is_ok()
            && AuditApprover::try_from(self.approver).is_ok()
            && AuditResult::try_from(self.result).is_ok()
    }

    fn into_wire(self) -> AgentAuditRecord {
        AgentAuditRecord {
            id: self.id,
            received_at_unix_millis: self.received_at_unix_millis,
            client_ref: self.client_ref,
            operation: self.operation,
            arguments: self.arguments,
            decision: self.decision,
            approver: self.approver,
            result: self.result,
        }
    }
}

impl Store {
    pub(super) fn new(paths: MarsaPaths, uid: u32) -> Self {
        Self {
            paths,
            uid,
            lock: Mutex::new(()),
            segment_bytes: FILE_LIMIT,
        }
    }

    pub(super) fn check(&self) -> io::Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("agent audit lock poisoned"))?;
        let directory = platform::state(self.paths.root(), self.uid)?;
        for name in &FILES[1..] {
            platform::existing(&directory, name, self.uid, self.segment_bytes)?;
        }
        if platform::existing(&directory, FILES[0], self.uid, self.segment_bytes)?.is_none() {
            platform::create(&directory, FILES[0])?;
        }
        Ok(())
    }

    pub(super) fn append(&self, record: Record) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        bytes.push(b'\n');
        if bytes.len() > RECORD_LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("agent audit lock poisoned"))?;
        let directory = platform::state(self.paths.root(), self.uid)?;
        let mut file = match platform::existing(&directory, FILES[0], self.uid, self.segment_bytes)?
        {
            Some(mut file) => {
                let size = file.metadata()?.len();
                if size + bytes.len() as u64 > self.segment_bytes
                    || !platform::clean_end(&mut file, size)?
                {
                    platform::rotate(&directory, FILES, self.uid, self.segment_bytes)?;
                    platform::create(&directory, FILES[0])?
                } else {
                    file
                }
            }
            None => platform::create(&directory, FILES[0])?,
        };
        file.write_all(&bytes)?;
        file.sync_data()
    }

    pub(super) fn list(&self, filter: ListAgentAuditRequest) -> io::Result<ListAgentAuditResponse> {
        if filter
            .client_ref
            .as_ref()
            .is_some_and(|value| !valid_hex(value, 64))
            || filter
                .cursor
                .as_ref()
                .is_some_and(|value| !valid_hex(value, 32))
            || filter.limit as usize > PAGE_LIMIT
            || filter.since_unix_millis.is_some_and(|value| value < 0)
            || filter
                .operation
                .is_some_and(|value| AgentOperation::try_from(value).is_err())
            || filter
                .decision
                .is_some_and(|value| AuditDecision::try_from(value).is_err())
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let limit = if filter.limit == 0 {
            DEFAULT_PAGE
        } else {
            filter.limit as usize
        };
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("agent audit lock poisoned"))?;
        let directory = platform::state(self.paths.root(), self.uid)?;
        let mut cursor_pending = filter.cursor.is_some();
        let mut records = Vec::new();
        for name in FILES {
            let Some(file) = platform::existing(&directory, name, self.uid, self.segment_bytes)?
            else {
                continue;
            };
            let mut bytes = Vec::new();
            file.take(self.segment_bytes).read_to_end(&mut bytes)?;
            // Ignore only a torn, unterminated last write. New entries rotate it away.
            let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
                continue;
            };
            for line in bytes[..end].rsplit(|byte| *byte == b'\n') {
                if line.len() > RECORD_LIMIT {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let record: Record = serde_json::from_slice(line).map_err(io::Error::other)?;
                if !record.valid() {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                if cursor_pending {
                    cursor_pending = filter.cursor.as_deref() != Some(&record.id);
                    continue;
                }
                if filter
                    .client_ref
                    .as_ref()
                    .is_some_and(|value| Some(value) != record.client_ref.as_ref())
                    || filter
                        .operation
                        .is_some_and(|value| record.operation != value)
                    || filter
                        .decision
                        .is_some_and(|value| record.decision != value)
                    || filter
                        .since_unix_millis
                        .is_some_and(|value| record.received_at_unix_millis < value)
                {
                    continue;
                }
                if records.len() == limit {
                    return Ok(ListAgentAuditResponse {
                        next_cursor: records
                            .last()
                            .map(|record: &AgentAuditRecord| record.id.clone()),
                        records,
                    });
                }
                records.push(record.into_wire());
            }
        }
        if cursor_pending {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(ListAgentAuditResponse {
            records,
            next_cursor: None,
        })
    }
}

fn valid_hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    };

    use super::*;

    fn fixture() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let paths = MarsaPaths::from_root(directory.path()).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(paths.state_dir())
            .unwrap();
        let store = Store::new(paths, nix::unistd::geteuid().as_raw());
        (directory, store)
    }

    #[test]
    fn rotation_keeps_only_four_private_segments_and_expires_old_cursors() {
        let (_directory, mut store) = fixture();
        store.segment_bytes = 300;
        let mut written = Vec::new();
        for _ in 0..7 {
            let record = Record::new(AgentOperation::StopContainer).unwrap();
            written.push(record.id.clone());
            store.append(record).unwrap();
        }
        let page = store.list(ListAgentAuditRequest::default()).unwrap();
        let actual = page
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            written.iter().rev().take(FILES.len()).collect::<Vec<_>>()
        );
        for segment in FILES {
            let metadata = fs::metadata(store.paths.state_dir().join(segment)).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            assert!(metadata.len() <= store.segment_bytes);
        }
        let error = store
            .list(ListAgentAuditRequest {
                cursor: Some(written[0].clone()),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_torn_suffix_rotates_and_preserves_prior_records() {
        let (_directory, store) = fixture();
        let first = Record::new(AgentOperation::StopContainer).unwrap();
        let first_id = first.id.clone();
        store.append(first).unwrap();
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(store.paths.state_dir().join(FILES[0]))
            .unwrap();
        file.write_all(b"{\"incomplete\":true").unwrap();
        let last = Record::new(AgentOperation::EngineStatus).unwrap();
        let last_id = last.id.clone();
        store.append(last).unwrap();
        let page = store.list(ListAgentAuditRequest::default()).unwrap();
        let actual = page
            .records
            .into_iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();
        assert_eq!(actual, [last_id, first_id]);
    }

    #[test]
    fn an_unsafe_audit_path_is_never_followed() {
        let (directory, store) = fixture();
        let other = directory.path().join("unrelated");
        fs::write(&other, b"original").unwrap();
        symlink(&other, store.paths.state_dir().join(FILES[0])).unwrap();
        assert!(store.check().is_err());
        assert!(
            store
                .append(Record::new(AgentOperation::StopContainer).unwrap())
                .is_err()
        );
        assert_eq!(fs::read(other).unwrap(), b"original");
    }
}
