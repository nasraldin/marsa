//! Decode media framing incrementally and close the HTTP task with its reader.

use http_body_util::BodyExt;
use hyper::body::{Buf as _, Bytes, Incoming};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::task::AbortHandle;

use super::{DockerChange, DockerError, StatsSample};

const RECORD_LIMIT: usize = 1024 * 1024;

pub(super) struct ConnectionTask(AbortHandle);

impl ConnectionTask {
    pub(super) fn new(handle: AbortHandle) -> Self {
        Self(handle)
    }
}

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) struct ResponseBody {
    pub(super) body: Incoming,
    pending: Bytes,
    _connection: ConnectionTask,
}

impl ResponseBody {
    pub(super) fn new(body: Incoming, connection: ConnectionTask) -> Self {
        Self {
            body,
            pending: Bytes::new(),
            _connection: connection,
        }
    }

    async fn fill(&mut self) -> Result<bool, DockerError> {
        while self.pending.is_empty() {
            let Some(frame) = self.body.frame().await.transpose()? else {
                return Ok(false);
            };
            if let Ok(data) = frame.into_data() {
                self.pending = data;
            }
        }
        Ok(true)
    }

    async fn take(&mut self, amount: usize) -> Result<Option<Vec<u8>>, DockerError> {
        if amount > RECORD_LIMIT {
            return Err(DockerError::RecordLimit);
        }
        let mut data = Vec::with_capacity(amount);
        while data.len() < amount {
            if !self.fill().await? {
                return if data.is_empty() {
                    Ok(None)
                } else {
                    Err(DockerError::LogFraming)
                };
            }
            let count = self.pending.len().min(amount - data.len());
            data.extend_from_slice(&self.pending.split_to(count));
        }
        Ok(Some(data))
    }

    pub(super) async fn json<T: DeserializeOwned>(
        &mut self,
        line: &mut Vec<u8>,
    ) -> Result<Option<T>, DockerError> {
        loop {
            let complete = if self.fill().await? {
                consume_line(&mut self.pending, line)?
            } else if line.is_empty() {
                return Ok(None);
            } else {
                true
            };
            if complete {
                if line.iter().all(u8::is_ascii_whitespace) {
                    line.clear();
                    continue;
                }
                let decoded = serde_json::from_slice(line);
                line.clear();
                return Ok(Some(decoded?));
            }
        }
    }
}

fn consume_line(bytes: &mut Bytes, line: &mut Vec<u8>) -> Result<bool, DockerError> {
    let newline = bytes.iter().position(|&byte| byte == b'\n');
    let length = newline.unwrap_or(bytes.len());
    if length > RECORD_LIMIT - line.len() {
        return Err(DockerError::RecordLimit);
    }
    line.extend_from_slice(&bytes.split_to(length));
    if newline.is_some() {
        bytes.advance(1);
    }
    Ok(newline.is_some())
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "Type")]
    kind: String,
}

/// Live Docker events relevant to the desktop snapshot.
pub struct DockerEvents {
    source: ResponseBody,
    line: Vec<u8>,
}

impl DockerEvents {
    pub(super) fn new(source: ResponseBody) -> Self {
        Self {
            source,
            line: Vec::new(),
        }
    }

    /// Read the next event that invalidates a container or image collection.
    pub async fn next(&mut self) -> Result<Option<DockerChange>, DockerError> {
        while let Some(event) = self.source.json::<Event>(&mut self.line).await? {
            match event.kind.as_str() {
                "container" => return Ok(Some(DockerChange::Containers)),
                "image" => return Ok(Some(DockerChange::Images)),
                _ => {}
            }
        }
        Ok(None)
    }
}

/// Live Docker container statistics.
pub struct DockerStats {
    source: ResponseBody,
    line: Vec<u8>,
}

impl DockerStats {
    pub(super) fn new(source: ResponseBody) -> Self {
        Self {
            source,
            line: Vec::new(),
        }
    }

    /// Read the next bounded sample.
    pub async fn next(&mut self) -> Result<Option<StatsSample>, DockerError> {
        self.source.json(&mut self.line).await
    }
}

/// Which container output produced a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    /// Container standard output.
    Stdout,
    /// Container standard error.
    Stderr,
    /// A TTY's combined output.
    Terminal,
}

impl LogSource {
    const ALL: [Self; 3] = [Self::Stdout, Self::Stderr, Self::Terminal];

    const fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
            Self::Terminal => 2,
        }
    }
}

/// One logical line of container output with its original stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Output that produced this line.
    pub source: LogSource,
    /// Text with invalid UTF-8 replaced and the line terminator removed.
    pub text: String,
}

/// A live Docker log stream with bounded framing and per-output line assembly.
pub struct DockerLogs {
    body: ResponseBody,
    framed: bool,
    payload: Bytes,
    source: LogSource,
    lines: [Vec<u8>; 3],
}

impl DockerLogs {
    pub(super) fn new(body: ResponseBody, framed: bool) -> Self {
        Self {
            body,
            framed,
            payload: Bytes::new(),
            source: LogSource::Terminal,
            lines: Default::default(),
        }
    }

    /// Read the next complete line, flushing any unterminated text at EOF.
    pub async fn next(&mut self) -> Result<Option<LogLine>, DockerError> {
        loop {
            if !self.payload.is_empty()
                && consume_line(&mut self.payload, &mut self.lines[self.source.index()])?
            {
                return Ok(Some(self.line(self.source)));
            }
            if self.payload.is_empty() && !self.read_payload().await? {
                return Ok(LogSource::ALL
                    .into_iter()
                    .find(|source| !self.lines[source.index()].is_empty())
                    .map(|source| self.line(source)));
            }
        }
    }

    fn line(&mut self, source: LogSource) -> LogLine {
        let bytes = &mut self.lines[source.index()];
        let text = String::from_utf8_lossy(bytes.strip_suffix(b"\r").unwrap_or(bytes)).into_owned();
        bytes.clear();
        LogLine { source, text }
    }

    async fn read_payload(&mut self) -> Result<bool, DockerError> {
        if !self.framed {
            if !self.body.fill().await? {
                return Ok(false);
            }
            self.payload = std::mem::take(&mut self.body.pending);
            return Ok(true);
        }
        let Some(header) = self.body.take(8).await? else {
            return Ok(false);
        };
        self.source = match header.as_slice() {
            [1, 0, 0, 0, ..] => LogSource::Stdout,
            [2, 0, 0, 0, ..] => LogSource::Stderr,
            _ => return Err(DockerError::LogFraming),
        };
        let length = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        self.payload = self
            .body
            .take(length)
            .await?
            .ok_or(DockerError::LogFraming)?
            .into();
        Ok(true)
    }
}
