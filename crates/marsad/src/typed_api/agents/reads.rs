//! Execute only fixed local Docker GET routes and project bounded observational data.

use std::{io, time::Duration};

use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode, body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use marsa_proto::host::v1::{AgentOperation, AgentOutput, AuditArguments};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::timeout;

use super::TypedApi;

const STARTUP_LIMIT: Duration = Duration::from_secs(180);
const REQUEST_LIMIT: Duration = Duration::from_secs(8);
const JSON_BYTES: usize = 1024 * 1024;
const LOG_BYTES: usize = 12 * 1024;
const DATA_BYTES: usize = 32 * 1024;
const MAX_CURSOR: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Read {
    operation: AgentOperation,
    arguments: Arguments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Arguments {
    Page {
        offset: usize,
        limit: usize,
    },
    Target(String),
    Logs {
        id: String,
        tail: u16,
        before: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default = "page_size")]
    limit: usize,
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Logs {
    id: String,
    #[serde(default = "tail_size")]
    tail: u16,
    cursor: Option<String>,
}

const fn page_size() -> usize {
    20
}
const fn tail_size() -> u16 {
    50
}

impl Read {
    pub(super) const fn supports(operation: AgentOperation) -> bool {
        use AgentOperation::*;
        matches!(
            operation,
            ListContainers
                | InspectContainer
                | ContainerLogs
                | ContainerStats
                | ListImages
                | InspectImage
                | ListVolumes
                | ListNetworks
        )
    }

    pub(super) fn parse(operation: AgentOperation, json: &str) -> Option<Self> {
        use AgentOperation::*;
        let arguments = match operation {
            ListContainers | ListImages | ListVolumes | ListNetworks => {
                let page: Page = serde_json::from_str(json).ok()?;
                let offset = match page.cursor {
                    Some(value)
                        if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) =>
                    {
                        value.parse::<usize>().ok()?
                    }
                    Some(_) => return None,
                    None => 0,
                };
                if offset >= MAX_CURSOR || !(1..=50).contains(&page.limit) {
                    return None;
                }
                Arguments::Page {
                    offset,
                    limit: page.limit,
                }
            }
            InspectContainer | InspectImage | ContainerStats => {
                let target: Target = serde_json::from_str(json).ok()?;
                let hex = if operation == InspectImage {
                    target.id.strip_prefix("sha256:")?
                } else {
                    &target.id
                };
                if !full_id(hex) {
                    return None;
                }
                Arguments::Target(target.id)
            }
            ContainerLogs => {
                let logs: Logs = serde_json::from_str(json).ok()?;
                if !full_id(&logs.id)
                    || !(1..=200).contains(&logs.tail)
                    || logs
                        .cursor
                        .as_deref()
                        .is_some_and(|value| !timestamp(value))
                {
                    return None;
                }
                Arguments::Logs {
                    id: logs.id,
                    tail: logs.tail,
                    before: logs.cursor,
                }
            }
            _ => return None,
        };
        Some(Self {
            operation,
            arguments,
        })
    }

    pub(super) const fn operation(&self) -> AgentOperation {
        self.operation
    }

    pub(super) const fn audit_arguments(&self) -> AuditArguments {
        match self.arguments {
            Arguments::Page { .. } => AuditArguments::PageOptionsRedacted,
            Arguments::Target(_) if matches!(self.operation, AgentOperation::InspectImage) => {
                AuditArguments::ImageTargetRedacted
            }
            Arguments::Target(_) => AuditArguments::ContainerTargetRedacted,
            Arguments::Logs { .. } => AuditArguments::ContainerLogOptionsRedacted,
        }
    }

    pub(super) async fn run(self, api: &TypedApi) -> io::Result<AgentOutput> {
        use AgentOperation::*;
        match self.arguments {
            Arguments::Page { offset, limit } => {
                let route = match self.operation {
                    ListContainers => format!(
                        "/v1.57/containers/json?all=1&limit={}",
                        (offset + limit + 1).min(MAX_CURSOR)
                    ),
                    ListImages => "/v1.57/images/json".into(),
                    ListVolumes => "/v1.57/volumes".into(),
                    ListNetworks => "/v1.57/networks".into(),
                    _ => return invalid(),
                };
                let (body, capped) = fetch(api, route, JSON_BYTES).await?;
                if capped {
                    return result(json!({"items":[]}), true, None);
                }
                let value: Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
                let rows = match self.operation {
                    ListVolumes => value.get("Volumes").and_then(Value::as_array),
                    _ => value.as_array(),
                };
                page(
                    rows.map(Vec::as_slice).unwrap_or_default(),
                    self.operation,
                    offset,
                    limit,
                )
            }
            Arguments::Target(id) => match self.operation {
                InspectContainer => {
                    let value = inspect(api, "containers", &id).await?;
                    result(
                        project(
                            &value,
                            &[
                                "Id",
                                "Name",
                                "Image",
                                "Created",
                                "RestartCount",
                                "Platform",
                                "State.Status",
                                "State.Running",
                                "State.ExitCode",
                                "Config.Image",
                                "Config.Tty",
                            ],
                        ),
                        false,
                        None,
                    )
                }
                InspectImage => {
                    let value = inspect(api, "images", &id).await?;
                    result(
                        project(
                            &value,
                            &["Id", "Created", "Architecture", "Os", "Size", "RepoTags"],
                        ),
                        false,
                        None,
                    )
                }
                ContainerStats => {
                    inspect(api, "containers", &id).await?;
                    let (body, capped) = fetch(
                        api,
                        format!("/v1.57/containers/{id}/stats?stream=false&one-shot=true"),
                        JSON_BYTES,
                    )
                    .await?;
                    if capped {
                        return result(json!({}), true, None);
                    }
                    let value: Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
                    result(
                        project(
                            &value,
                            &[
                                "id",
                                "read",
                                "cpu_stats.cpu_usage.total_usage",
                                "cpu_stats.system_cpu_usage",
                                "memory_stats.usage",
                                "memory_stats.limit",
                                "pids_stats.current",
                                "pids_stats.limit",
                            ],
                        ),
                        false,
                        None,
                    )
                }
                _ => invalid(),
            },
            Arguments::Logs { id, tail, before } => {
                let value = inspect(api, "containers", &id).await?;
                let tty = value
                    .pointer("/Config/Tty")
                    .and_then(Value::as_bool)
                    .ok_or(io::ErrorKind::InvalidData)?;
                let mut route = format!(
                    "/v1.57/containers/{id}/logs?stdout=true&stderr=true&timestamps=true&tail={}",
                    u32::from(tail) + 1
                );
                if let Some(before) = before {
                    route.push_str("&until=");
                    route.push_str(&before);
                }
                let (body, capped) = fetch(api, route, LOG_BYTES).await?;
                let payload = if tty {
                    body
                } else {
                    demultiplex(&body, capped)?
                };
                log_page(&payload, usize::from(tail), capped)
            }
        }
    }
}

fn full_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !matches!(bytes.len(), 20 | 22..=30) || bytes.last() != Some(&b'Z') {
        return false;
    }
    bytes[..bytes.len() - 1]
        .iter()
        .enumerate()
        .all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'.',
            _ => byte.is_ascii_digit(),
        })
}

async fn inspect(api: &TypedApi, collection: &'static str, id: &str) -> io::Result<Value> {
    let (body, capped) = fetch(api, format!("/v1.57/{collection}/{id}/json"), JSON_BYTES).await?;
    if capped {
        return invalid();
    }
    let value: Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
    if value.get("Id").and_then(Value::as_str) != Some(id) {
        return Err(io::ErrorKind::NotFound.into());
    }
    Ok(value)
}

async fn fetch(api: &TypedApi, route: String, max: usize) -> io::Result<(Vec<u8>, bool)> {
    let stream = timeout(STARTUP_LIMIT, api.connect_docker()).await??;
    timeout(REQUEST_LIMIT, async {
        let (mut sender, connection) = http1::Builder::new()
            .max_buf_size(16 * 1024)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(io::Error::other)?;
        let request = async move {
            let mut response = sender
                .send_request(
                    Request::get(route)
                        .header("Host", "docker")
                        .header("Connection", "close")
                        .body(Empty::<Bytes>::new())
                        .map_err(io::Error::other)?,
                )
                .await
                .map_err(io::Error::other)?;
            match response.status() {
                StatusCode::OK => {}
                StatusCode::NOT_FOUND => return Err(io::ErrorKind::NotFound.into()),
                _ => return invalid(),
            }
            let mut body = Vec::new();
            while let Some(frame) = response.body_mut().frame().await {
                let frame = frame.map_err(io::Error::other)?;
                if let Some(data) = frame.data_ref() {
                    let available = max - body.len();
                    body.extend_from_slice(&data[..data.len().min(available)]);
                    if data.len() > available {
                        return Ok((body, true));
                    }
                }
            }
            Ok((body, false))
        };
        tokio::pin!(request, connection);
        tokio::select! {
            value = &mut request => value,
            _ = &mut connection => request.await,
        }
    })
    .await?
}

fn project(input: &Value, keys: &[&str]) -> Value {
    let mut fields = serde_json::Map::new();
    for key in keys {
        let value = key.split('.').try_fold(input, Value::get);
        match value {
            Some(Value::String(text)) => {
                fields.insert((*key).into(), bounded_text(text, 256).into());
            }
            Some(Value::Bool(_) | Value::Number(_)) => {
                fields.insert((*key).into(), value.cloned().unwrap_or_default());
            }
            Some(Value::Array(items)) if ["Names", "RepoTags"].contains(key) => {
                fields.insert(
                    (*key).into(),
                    Value::Array(
                        items
                            .iter()
                            .take(4)
                            .filter_map(Value::as_str)
                            .map(|text| bounded_text(text, 96).into())
                            .collect(),
                    ),
                );
            }
            _ => {}
        }
    }
    Value::Object(fields)
}

fn bounded_text(value: &str, bytes: usize) -> &str {
    &value[..value.floor_char_boundary(value.len().min(bytes))]
}

fn page(
    rows: &[Value],
    operation: AgentOperation,
    offset: usize,
    limit: usize,
) -> io::Result<AgentOutput> {
    use AgentOperation::*;
    let keys: &[&str] = match operation {
        ListContainers => &[
            "Id", "Names", "Image", "ImageID", "State", "Status", "Created",
        ],
        ListImages => &["Id", "RepoTags", "Size", "Created"],
        ListVolumes => &["Name", "Driver", "Scope", "CreatedAt"],
        ListNetworks => &["Id", "Name", "Driver", "Scope", "Internal", "Created"],
        _ => return invalid(),
    };
    let mut items = Vec::new();
    let mut remaining = DATA_BYTES - 64;
    for row in rows.iter().take(MAX_CURSOR).skip(offset).take(limit) {
        let row = project(row, keys);
        let size = serde_json::to_vec(&row).map_err(io::Error::other)?.len() + 1;
        if remaining < size {
            break;
        }
        remaining -= size;
        items.push(row);
    }
    let next = offset + items.len();
    result(
        json!({"items":items}),
        next < rows.len() || next >= MAX_CURSOR,
        (next < rows.len().min(MAX_CURSOR)).then(|| next.to_string()),
    )
}

fn demultiplex(input: &[u8], truncated: bool) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut remaining = input;
    while !remaining.is_empty() {
        let Some((header, data)) = remaining.split_at_checked(8) else {
            return if truncated { Ok(output) } else { invalid() };
        };
        if !matches!(header[0], 1 | 2) || header[1..4] != [0; 3] {
            return invalid();
        }
        let size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let Some((payload, rest)) = data.split_at_checked(size) else {
            if truncated {
                output.extend_from_slice(data);
                return Ok(output);
            }
            return invalid();
        };
        output.extend_from_slice(payload);
        remaining = rest;
    }
    Ok(output)
}

fn log_page(data: &[u8], tail: usize, capped: bool) -> io::Result<AgentOutput> {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<_> = text.lines().collect();
    let mut selected = Vec::new();
    let mut remaining = DATA_BYTES - 64;
    for line in lines.iter().rev().take(tail) {
        let size = serde_json::to_vec(line).map_err(io::Error::other)?.len() + 1;
        if size > remaining {
            break;
        }
        remaining -= size;
        selected.push(*line);
    }
    let truncated = capped || selected.len() < lines.len();
    selected.reverse();
    let next = if truncated {
        selected
            .first()
            .and_then(|line| line.split_once(' '))
            .map(|(at, _)| at)
            .filter(|at| timestamp(at))
            .map(str::to_owned)
    } else {
        None
    };
    result(json!({"lines":selected}), truncated, next)
}

fn result(data: Value, truncated: bool, next_cursor: Option<String>) -> io::Result<AgentOutput> {
    let data_json = serde_json::to_string(&data).map_err(io::Error::other)?;
    if data_json.len() > DATA_BYTES {
        return invalid();
    }
    Ok(AgentOutput {
        data_json,
        truncated,
        next_cursor,
    })
}

fn invalid<T>() -> io::Result<T> {
    Err(io::ErrorKind::InvalidData.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_docker_read_marks_its_schema_without_storing_arguments() {
        let container = "a".repeat(64);
        let target = json!({"id":container}).to_string();
        let image = json!({"id":format!("sha256:{container}")}).to_string();
        let logs = json!({"id":container,"tail":12,"cursor":"2026-07-28T01:00:00Z"}).to_string();
        let page = r#"{"cursor":"99","limit":2}"#;
        for (operation, args, shape) in [
            (
                AgentOperation::ListContainers,
                page,
                AuditArguments::PageOptionsRedacted,
            ),
            (
                AgentOperation::ListImages,
                page,
                AuditArguments::PageOptionsRedacted,
            ),
            (
                AgentOperation::ListNetworks,
                page,
                AuditArguments::PageOptionsRedacted,
            ),
            (
                AgentOperation::ListVolumes,
                page,
                AuditArguments::PageOptionsRedacted,
            ),
            (
                AgentOperation::InspectContainer,
                &target,
                AuditArguments::ContainerTargetRedacted,
            ),
            (
                AgentOperation::ContainerStats,
                &target,
                AuditArguments::ContainerTargetRedacted,
            ),
            (
                AgentOperation::InspectImage,
                &image,
                AuditArguments::ImageTargetRedacted,
            ),
            (
                AgentOperation::ContainerLogs,
                &logs,
                AuditArguments::ContainerLogOptionsRedacted,
            ),
        ] {
            assert!(Read::supports(operation));
            assert_eq!(
                Read::parse(operation, args).unwrap().audit_arguments(),
                shape
            );
        }
    }

    #[test]
    fn bounded_lists_drop_unrequested_metadata_and_page_live_results() {
        let items = vec![
            json!({"Id":"first","Labels":{"credential":"not-returned"}}),
            json!({"Id":"second"}),
        ];
        let first = page(&items, AgentOperation::ListContainers, 0, 1).unwrap();
        assert!(first.truncated);
        assert_eq!(first.next_cursor.as_deref(), Some("1"));
        assert_eq!(first.data_json, r#"{"items":[{"Id":"first"}]}"#);
        let second = page(&items, AgentOperation::ListContainers, 1, 1).unwrap();
        assert!(!second.truncated);
        assert_eq!(second.data_json, r#"{"items":[{"Id":"second"}]}"#);
    }

    #[test]
    fn multiplexed_logs_keep_a_cursor_and_report_truncated_bytes() {
        let payload = b"2026-07-28T01:00:00Z first\n2026-07-28T01:00:01Z second\n";
        let mut stream = vec![1, 0, 0, 0];
        stream.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        stream.extend_from_slice(payload);
        let decoded = demultiplex(&stream, false).unwrap();
        let output = log_page(&decoded, 1, false).unwrap();
        assert!(output.truncated);
        assert_eq!(output.next_cursor.as_deref(), Some("2026-07-28T01:00:01Z"));
        assert!(output.data_json.contains("second"));
        let clipped = demultiplex(&stream[..stream.len() - 2], true).unwrap();
        assert!(log_page(&clipped, 3, true).unwrap().truncated);
        assert!(demultiplex(&stream[..stream.len() - 2], false).is_err());
    }
}
