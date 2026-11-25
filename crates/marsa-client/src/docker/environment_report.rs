//! Read-only inventory and event summaries for the selected Docker environment.

use http_body_util::{BodyExt, Limited};
use hyper::Method;
use marsa_proto::host::{
    DOCKER_ACTIVITY_WINDOW_SECONDS, docker_activity_query,
    v1::{DockerOverviewRequest, DockerOverviewSection},
};
use serde::Deserialize;
use tokio::time::timeout;

use crate::{ClientError, MarsaClient};

use super::{
    ContainerId, ContainerSummary, DockerClient, DockerError, REQUEST_TIMEOUT, SNAPSHOT_LIMIT,
};

const ACTIVITY_LIMIT: usize = 256;
const EVENT_QUERY: &str = "/events?since=";

/// A recent event supplied by the selected Docker engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerActivity {
    /// Docker resource type, such as `container`, `network`, or `image`.
    pub kind: String,
    /// Engine action code, excluding free-form command or health output.
    pub action: String,
    /// Resource name, falling back to the engine's actor identifier.
    pub subject: String,
    /// Event time in Unix seconds.
    pub timestamp: i64,
    time_nano: i64,
    unhealthy: bool,
}

impl DockerActivity {
    /// Whether this record is a Docker container transition to unhealthy.
    pub fn is_unhealthy(&self) -> bool {
        self.unhealthy
    }

    /// A high resolution timestamp, falling back to the event second when absent.
    pub fn timestamp_nanos(&self) -> i64 {
        const NANOS_PER_SECOND: i64 = 1_000_000_000;
        if self.time_nano.div_euclid(NANOS_PER_SECOND) == self.timestamp {
            self.time_nano
        } else {
            self.timestamp.saturating_mul(NANOS_PER_SECOND)
        }
    }
}

/// A shareable projection of the currently available Docker inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentReport {
    /// Known containers, with only name, image and lifecycle state included.
    pub containers: Vec<EnvironmentReportContainer>,
    /// Number of images observed in the inventory.
    pub image_count: usize,
    /// Number of volumes, or `None` if the inventory could not read them.
    pub volume_count: Option<usize>,
    /// Number of networks, or `None` if the inventory could not read them.
    pub network_count: Option<usize>,
}

/// Container metadata included in an environment report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentReportContainer {
    /// Docker name without its leading slash, or the short ID when unnamed.
    pub name: String,
    /// Image reference reported by Docker.
    pub image: String,
    /// Lifecycle state reported by Docker.
    pub state: String,
}

impl EnvironmentReport {
    /// Project an already-loaded inventory without requesting container details.
    pub fn from_inventory(
        containers: &[ContainerSummary],
        image_count: usize,
        volume_count: Option<usize>,
        network_count: Option<usize>,
    ) -> Self {
        Self {
            containers: containers
                .iter()
                .map(|container| {
                    let name = container
                        .names
                        .first()
                        .map(|name| name.trim_start_matches('/'))
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&container.id.as_str()[..12]);
                    EnvironmentReportContainer {
                        name: display_value(name, 255),
                        image: display_value(&container.image, 512),
                        state: display_value(&container.state, 64),
                    }
                })
                .collect(),
            image_count,
            volume_count,
            network_count,
        }
    }
}

impl DockerClient {
    /// Read up to 256 events that the engine retained from the last 24 hours, newest first.
    pub async fn recent_activity(&self) -> Result<Vec<DockerActivity>, DockerError> {
        self.recent_activity_with_hours(24).await
    }

    /// Read retained events using a supported lookback of 1, 6 or 24 hours.
    pub async fn recent_activity_with_hours(
        &self,
        hours: u32,
    ) -> Result<Vec<DockerActivity>, DockerError> {
        let path = docker_activity_query(time::OffsetDateTime::now_utc().unix_timestamp(), hours)
            .ok_or(DockerError::InvalidActivityHours)?;
        let bytes = timeout(REQUEST_TIMEOUT, async {
            let (_, response) = self.request(Method::GET, &path).await?;
            Limited::new(response.body, SNAPSHOT_LIMIT)
                .collect()
                .await
                .map(|body| body.to_bytes())
                .map_err(DockerError::Snapshot)
        })
        .await
        .map_err(|_| DockerError::Timeout)??;

        activity_records(&bytes)
    }
}

impl MarsaClient {
    /// Read recent activity through the owner API without waking the managed engine.
    pub async fn recent_docker_activity(
        &self,
        hours: u32,
    ) -> Result<Vec<DockerActivity>, DockerError> {
        let mut request = tonic::Request::new(DockerOverviewRequest {
            section: DockerOverviewSection::Activity.into(),
            activity_hours: Some(hours),
            ..Default::default()
        });
        request.set_timeout(REQUEST_TIMEOUT);
        let response = self
            .stats()
            .docker_overview(request)
            .await
            .map_err(ClientError::from)?
            .into_inner();
        activity_records(&response.json)
    }
}

fn activity_records(bytes: &[u8]) -> Result<Vec<DockerActivity>, DockerError> {
    let mut events = serde_json::Deserializer::from_slice(bytes)
        .into_iter::<EngineEvent>()
        .collect::<Result<Vec<_>, _>>()?;
    events.reverse();
    events.sort_by_key(|event| std::cmp::Reverse((event.time, event.time_nano)));
    events.truncate(ACTIVITY_LIMIT);
    Ok(events.into_iter().map(EngineEvent::project).collect())
}

pub(super) fn finite_activity_route(path: &str) -> bool {
    let Some((since, until)) = path
        .strip_prefix(EVENT_QUERY)
        .and_then(|q| q.split_once("&until="))
    else {
        return false;
    };
    let decimal_seconds = |value: &str| {
        value
            .parse::<i64>()
            .ok()
            .filter(|_| value.bytes().all(|byte| byte.is_ascii_digit()))
    };
    matches!((decimal_seconds(since), decimal_seconds(until)), (Some(since), Some(until))
        if since > 0 && until > since && until - since <= DOCKER_ACTIVITY_WINDOW_SECONDS)
}

#[derive(Deserialize)]
struct EngineEvent {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Actor")]
    actor: EngineActor,
    time: i64,
    #[serde(rename = "timeNano", default)]
    time_nano: i64,
}

impl EngineEvent {
    fn project(self) -> DockerActivity {
        // The action suffix may contain a command with arguments or free-form health output.
        let action = self
            .action
            .split_once(':')
            .map_or(self.action.as_str(), |(action, _)| action);
        let subject = self
            .actor
            .attributes
            .and_then(|attributes| attributes.name)
            .filter(|name| !name.is_empty());
        DockerActivity {
            kind: display_value(&self.kind, 64),
            action: display_value(action, 128),
            subject: display_value(subject.as_deref().unwrap_or(&self.actor.id), 512),
            timestamp: self.time,
            time_nano: self.time_nano,
            unhealthy: self.kind == "container"
                && self.action == "health_status: unhealthy"
                && ContainerId::parse(self.actor.id).is_ok(),
        }
    }
}

#[derive(Deserialize)]
struct EngineActor {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Attributes")]
    attributes: Option<EngineActorAttributes>,
}

#[derive(Deserialize)]
struct EngineActorAttributes {
    name: Option<String>,
}

pub(super) fn display_value(value: &str, limit: usize) -> String {
    value
        .chars()
        .take(limit)
        .map(|character| match character {
            '\u{061c}'
            | '\u{200e}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => ' ',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use hyper::Method;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use crate::{
        docker::{
            ContainerSummary, EnvironmentReport, ExternalEndpoint,
            inventory::{ExternalTransport, external_route},
        },
        platform,
    };

    #[test]
    fn external_policy_allows_a_finite_read_only_events_query() {
        for path in [
            "/events",
            "/events?until=1826919000",
            "/events?since=1826918999&until=1826919000&filters=%7B%7D",
            "/events?since=0&until=1826919000",
            "/events?since=1826919001&until=1826919000",
            "/events?since=1826818000&until=1826919000",
        ] {
            assert!(!external_route(&Method::GET, path, true, false), "{path}");
        }
        let path = "/events?since=1826918999&until=1826919000";
        assert!(external_route(&Method::GET, path, true, false));
        assert!(!external_route(&Method::GET, path, false, false));
        assert!(!external_route(&Method::GET, path, true, true));
        assert!(!external_route(&Method::POST, path, true, false));
    }

    #[tokio::test]
    async fn history_queries_the_pinned_socket_and_projects_recent_records() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let path = directory.path().join("docker.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let endpoint = ExternalEndpoint::managed(
            "managed:history-fixture".into(),
            ExternalTransport::Unix {
                identity: Some(platform::socket_identity(&path).await.unwrap()),
                socket: path,
            },
        );
        let clock = time::OffsetDateTime::now_utc().unix_timestamp();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            let request = String::from_utf8(request).unwrap();
            let query = request
                .lines()
                .next()
                .unwrap()
                .strip_prefix("GET /events?since=")
                .unwrap()
                .strip_suffix(" HTTP/1.1")
                .unwrap();
            let (since, until) = query.split_once("&until=").unwrap();
            let until = until.parse::<i64>().unwrap();
            assert!(until >= clock);
            assert_eq!(until - since.parse::<i64>().unwrap(), 3_600);
            let history = concat!(
                r#"{"Type":"container","Action":"start","Actor":{"ID":"9638f123","Attributes":{"name":"api","image":"nginx","label":"private"}},"scope":"local","time":1826918998,"timeNano":1826918998000000000}"#,
                "\n",
                r#"{"Type":"network","Action":"connect","Actor":{"ID":"c1a2f3","Attributes":{"name":"development\nnetwork","container":"9638f123"}},"scope":"local","time":1826918999,"timeNano":1826918999000000000}"#,
                "\n",
                r#"{"Type":"container","Action":"exec_start: tool private-argument","Actor":{"ID":"missing-name","Attributes":null},"scope":"local","time":1826918999,"timeNano":1826918999000000001}"#,
                "\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/jsonl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{history}",
                history.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let records = endpoint
            .management_client()
            .recent_activity_with_hours(1)
            .await
            .unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].action, "exec_start");
        assert_eq!(records[0].subject, "missing-name");
        assert_eq!(records[1].kind, "network");
        assert_eq!(records[1].action, "connect");
        assert_eq!(records[1].subject, "development network");
        assert_eq!(records[1].timestamp, 1826918999);
        assert_eq!(records[2].subject, "api");
        server.await.unwrap();
    }

    #[test]
    fn only_the_exact_unhealthy_container_action_can_trigger_an_alert() {
        let records = super::activity_records(br#"{"Type":"container","Action":"health_status: unhealthy","Actor":{"ID":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","Attributes":{"name":"public"}},"time":1826918999,"timeNano":1826918999000000001}
{"Type":"container","Action":"health_status: private unhealthy output","Actor":{"ID":"second","Attributes":null},"time":1826918998}
{"Type":"network","Action":"health_status: unhealthy","Actor":{"ID":"third","Attributes":null},"time":1826918997}
{"Type":"container","Action":"health_status: unhealthy","Actor":{"ID":"not-a-container-id","Attributes":null},"time":1826918996}"#).unwrap();
        assert!(records[0].is_unhealthy());
        assert_eq!(records[0].action, "health_status");
        assert!(!records[1].is_unhealthy());
        assert!(!records[2].is_unhealthy());
        assert!(!records[3].is_unhealthy());
    }

    #[test]
    fn report_uses_container_names_and_retains_unknown_inventory_counts() {
        let container: ContainerSummary = serde_json::from_value(serde_json::json!({
            "Id": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "Names": ["/development-api"],
            "Image": "example/api:local",
            "State": "running",
            "Status": "Up 2 minutes",
            "Created": 1826918999_i64,
            "ImageID": "sha256:l2m",
            "Labels": {"com.docker.compose.project": "development"}
        }))
        .unwrap();
        let report = EnvironmentReport::from_inventory(&[container], 2, None, Some(0));
        assert_eq!(report.image_count, 2);
        assert_eq!(report.volume_count, None);
        assert_eq!(report.network_count, Some(0));
        assert_eq!(report.containers[0].name, "development-api");
        assert_eq!(report.containers[0].image, "example/api:local");
        assert_eq!(report.containers[0].state, "running");
    }
}
