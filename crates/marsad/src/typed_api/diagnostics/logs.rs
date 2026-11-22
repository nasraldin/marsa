//! Follow classified daemon and guest records across appends and file rotation.

use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    time::Duration,
};

use futures_util::stream;
use marsa_core::paths::MarsaPaths;
use marsa_proto::host::v1::{LogEntry, LogsRequest};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{sync::watch, task::spawn_blocking, time::sleep};
use tonic::Status;

use super::{storage, summaries};
use crate::typed_api::events::Output;

const POLL: Duration = Duration::from_millis(250);
const MAX_LINES: usize = 256;

pub(super) fn stream(
    paths: MarsaPaths,
    uid: u32,
    closing: watch::Receiver<bool>,
    request: LogsRequest,
) -> Result<Output<LogEntry>, Status> {
    let limit = match request.tail_lines as usize {
        0 => 100,
        lines @ 1..=MAX_LINES => lines,
        _ => {
            return Err(Status::invalid_argument(
                "log tail exceeds the supported limit",
            ));
        }
    };
    let state = Reader {
        paths,
        uid,
        cursors: Default::default(),
    };
    let pending = VecDeque::new();
    let follow = request.follow;
    Ok(Box::pin(stream::unfold(
        Some((state, closing, pending, true)),
        move |state| async move {
            let (mut reader, mut closing, mut pending, mut initial) = state?;
            loop {
                if *closing.borrow() {
                    return None;
                }
                if let Some(entry) = pending.pop_front() {
                    return Some((Ok(entry), Some((reader, closing, pending, initial))));
                }
                if !initial {
                    if !follow {
                        return None;
                    }
                    tokio::select! {
                        _ = sleep(POLL) => (),
                        _ = closing.changed() => return None,
                    };
                }
                let count = if initial { limit } else { MAX_LINES };
                let result = spawn_blocking(move || {
                    let result = reader.poll(count);
                    (reader, result)
                })
                .await;
                match result {
                    Ok((state, Ok(entries))) => {
                        reader = state;
                        pending = entries;
                        initial = false;
                    }
                    Ok((_, Err(error))) if error.kind() == io::ErrorKind::PermissionDenied => {
                        return Some((
                            Err(Status::permission_denied("diagnostic logs are not private")),
                            None,
                        ));
                    }
                    Ok((_, Err(_))) | Err(_) => {
                        return Some((
                            Err(Status::internal("could not read diagnostic logs")),
                            None,
                        ));
                    }
                }
            }
        },
    )))
}

struct Reader {
    paths: MarsaPaths,
    uid: u32,
    cursors: [Cursor; 2],
}

impl Reader {
    fn poll(&mut self, count: usize) -> io::Result<VecDeque<LogEntry>> {
        let [daemon, guest] = storage::log_files(&self.paths, self.uid)?;
        let daemon = records(self.cursors[0].read(daemon)?, Source::Daemon, count);
        let guest = records(self.cursors[1].read(guest)?, Source::Guest, count);
        // Reserve room for both sources when the tails exceed the shared quota.
        let keep_daemon = daemon
            .len()
            .min((count / 2).max(count.saturating_sub(guest.len())));
        let keep_guest = guest.len().min(count - keep_daemon);
        Ok(daemon
            .into_iter()
            .rev()
            .take(keep_daemon)
            .rev()
            .chain(guest.into_iter().rev().take(keep_guest).rev())
            .collect())
    }
}

#[derive(Default)]
struct Cursor {
    file: Option<(u64, u64)>,
    offset: u64,
}

impl Cursor {
    fn read(&mut self, file: io::Result<File>) -> io::Result<Vec<u8>> {
        let mut file = match file {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        let identity = (metadata.dev(), metadata.ino());
        if self.file != Some(identity) || metadata.len() < self.offset {
            self.file = Some(identity);
            self.offset = 0;
        }
        let offset = self
            .offset
            .max(metadata.len().saturating_sub(storage::MAX_LOG_BYTES));
        let starts_mid_record = offset > self.offset;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(storage::MAX_LOG_BYTES).read_to_end(&mut bytes)?;
        let Some(last) = bytes.iter().rposition(|&byte| byte == b'\n') else {
            return Ok(Vec::new());
        };
        self.offset = offset + last as u64 + 1;
        bytes.truncate(last + 1);
        if starts_mid_record && let Some(first) = bytes.iter().position(|&byte| byte == b'\n') {
            bytes.drain(..=first);
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
enum Source {
    Daemon,
    Guest,
}

fn records(bytes: Vec<u8>, source: Source, count: usize) -> Vec<LogEntry> {
    let mut entries: Vec<_> = bytes
        .rsplit(|&byte| byte == b'\n')
        .filter_map(|line| {
            if line.is_empty() || line.len() > 4096 {
                return None;
            }
            let line = std::str::from_utf8(line).ok()?.trim_ascii_end();
            let (summary, component, timestamp) = match source {
                Source::Daemon => (
                    summaries::daemon(line)?,
                    "daemon",
                    daemon_time(line).unwrap_or(0),
                ),
                Source::Guest => {
                    if let Some(engine) = summaries::engine(line) {
                        (engine, "docker", 0)
                    } else {
                        (summaries::console(line)?, "guest", 0)
                    }
                }
            };
            let mut message = summary.get("event")?.as_str()?.to_owned();
            if let (Some(stage), Some(duration)) = (
                summary.get("stage").and_then(|value| value.as_str()),
                summary.get("duration_ms").and_then(|value| value.as_u64()),
            ) {
                message.push_str(&format!(" {stage} {duration}ms"));
            }
            Some(LogEntry {
                at_unix_millis: timestamp,
                component: component.to_owned(),
                level: summary
                    .get("level")
                    .and_then(|level| level.as_str())
                    .unwrap_or("info")
                    .to_owned(),
                message,
            })
        })
        .take(count)
        .collect();
    entries.reverse();
    entries
}

fn daemon_time(line: &str) -> Option<i64> {
    let log: serde_json::Value = serde_json::from_str(line).ok()?;
    let timestamp = OffsetDateTime::parse(log.get("timestamp")?.as_str()?, &Rfc3339).ok()?;
    i64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).ok()
}
