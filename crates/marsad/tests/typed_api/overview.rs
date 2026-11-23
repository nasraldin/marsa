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
