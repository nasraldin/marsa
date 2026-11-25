//! Exercise dashboard observation over the owner socket with the fake guest.

use marsa_proto::host::v1::{DockerOverviewRequest, DockerOverviewSection, stats_server::Stats};

use super::*;

#[tokio::test]
async fn overview_is_owner_only_and_stays_passive_across_engine_lifecycle() {
    let fixture = Fixture::with_docker(
        vec![AgentStep::Send(b"HR".to_vec())],
        vec![
            AgentStep::Expect(b"GET /v1.57/containers/json?all=true HTTP/1.1\r\n".to_vec()),
            AgentStep::Send(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_vec(),
            ),
        ],
    )
    .await;
    let request = DockerOverviewRequest {
        section: DockerOverviewSection::Containers.into(),
        container_id: String::new(),
        prefer_cache: false,
        activity_hours: None,
    };
    assert_eq!(
        fixture
            .api
            .docker_overview(Request::new(request.clone()))
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied,
    );
    let mut stats = StatsClient::new(fixture.channel.clone());
    for stopped in [true, false, true] {
        if stopped {
            assert_eq!(
                stats
                    .docker_overview(request.clone())
                    .await
                    .unwrap_err()
                    .code(),
                Code::FailedPrecondition,
            );
            assert_eq!(
                fixture.supervisor.snapshot().state,
                marsad::supervisor::State::Stopped
            );
        } else {
            fixture.supervisor.ensure_ready().await.unwrap();
            assert_eq!(
                stats
                    .docker_overview(request.clone())
                    .await
                    .unwrap()
                    .into_inner()
                    .json,
                b"[]",
            );
            fixture.supervisor.stop().await.unwrap();
        }
    }
}

#[tokio::test]
async fn prefer_cache_returns_last_snapshot_without_docker() {
    let fixture = Fixture::with_docker(
        vec![AgentStep::Send(b"HR".to_vec())],
        vec![
            AgentStep::Expect(b"GET /v1.57/containers/json?all=true HTTP/1.1\r\n".to_vec()),
            AgentStep::Send(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_vec(),
            ),
        ],
    )
    .await;
    fixture.supervisor.ensure_ready().await.unwrap();
    let mut stats = StatsClient::new(fixture.channel.clone());
    let live = DockerOverviewRequest {
        section: DockerOverviewSection::Containers.into(),
        container_id: String::new(),
        prefer_cache: false,
        activity_hours: None,
    };
    let first = stats.docker_overview(live).await.unwrap().into_inner();
    assert_eq!(first.json, b"[]");
    assert!(first.observed_at_unix_millis > 0);

    // Stop Docker script replies: a cache hit must not contact the guest again.
    fixture.supervisor.stop().await.unwrap();
    let cached = stats
        .docker_overview(DockerOverviewRequest {
            section: DockerOverviewSection::Containers.into(),
            container_id: String::new(),
            prefer_cache: true,
            activity_hours: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cached.json, b"[]");
    assert_eq!(
        cached.observed_at_unix_millis,
        first.observed_at_unix_millis
    );
}

#[tokio::test]
async fn docker_inventory_events_are_observed_without_preventing_idle_stop() {
    let event = b"{\"Type\":\"container\",\"Action\":\"start\"}\n";
    let headers =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n";
    let mut body = format!("{:x}\r\n", event.len()).into_bytes();
    body.extend_from_slice(event);
    body.extend_from_slice(b"\r\n");
    let fixture = Fixture::with_docker(
        vec![AgentStep::Send(b"HR".to_vec())],
        vec![
            AgentStep::Expect(b"GET /v1.57/events".to_vec()),
            AgentStep::Send(headers.to_vec()),
            AgentStep::Wait(Duration::from_millis(400)),
            AgentStep::Send(body),
            AgentStep::Wait(Duration::from_secs(10)),
        ],
    )
    .await;
    let mut events = fixture.events().await;
    let initial = next(&mut events).await;
    assert!(matches!(initial.body, Some(Body::Snapshot(_))));
    assert_eq!(
        fixture.supervisor.snapshot().state,
        marsad::supervisor::State::Stopped
    );

    let mut settings = SettingsClient::new(fixture.channel.clone());
    let revision = settings
        .get(GetSettingsRequest {})
        .await
        .unwrap()
        .into_inner()
        .revision;
    settings
        .update(UpdateSettingsRequest {
            request_id: action(1).request_id,
            expected_revision: revision,
            settings_toml: "[vm]\nidle_timeout = 1\n".into(),
        })
        .await
        .unwrap();
    fixture.supervisor.ensure_ready().await.unwrap();

    let changed = timeout(WAIT, async {
        let mut cursor = initial.sequence;
        let mut invalidations = 0;
        loop {
            let event = events.message().await.unwrap().unwrap();
            assert_eq!(event.sequence, cursor + 1);
            cursor = event.sequence;
            if matches!(event.body, Some(Body::DockerInventoryChanged(_))) {
                invalidations += 1;
                if invalidations == 2 {
                    return event;
                }
            }
        }
    })
    .await
    .expect("inventory invalidation");
    fixture.supervisor.report_containers(0).await.unwrap();
    let idle = next_state(&mut events, changed, EngineState::Idle).await;
    let stopping = next_state(&mut events, idle, EngineState::Stopping).await;
    next_state(&mut events, stopping, EngineState::Stopped).await;
}

#[tokio::test]
async fn recent_docker_activity_requires_an_already_running_vm() {
    let body =
        br#"{"Type":"container","Action":"start","Actor":{"ID":"fixture"},"time":1826918998}"#;
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let fixture = Fixture::with_docker(
        vec![AgentStep::Send(b"HR".to_vec())],
        vec![
            AgentStep::Expect(b"GET /v1.57/events?since=".to_vec()),
            AgentStep::Send(response),
        ],
    )
    .await;
    let mut stats = StatsClient::new(fixture.channel.clone());
    let request = DockerOverviewRequest {
        section: DockerOverviewSection::Activity.into(),
        prefer_cache: true,
        activity_hours: None,
        ..Default::default()
    };
    assert_eq!(
        stats
            .docker_overview(request.clone())
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(
        fixture.supervisor.snapshot().state,
        marsad::supervisor::State::Stopped
    );
    fixture.supervisor.ensure_ready().await.unwrap();
    assert_eq!(
        stats
            .docker_overview(request.clone())
            .await
            .unwrap()
            .into_inner()
            .json,
        body
    );
    fixture.supervisor.stop().await.unwrap();
    assert_eq!(
        stats.docker_overview(request).await.unwrap_err().code(),
        Code::FailedPrecondition,
        "activity is never served from the inventory cache"
    );
}
