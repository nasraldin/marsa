//! Verify the authenticated, non-mutating Docker inventory path.

use marsa_proto::host::v1::{
    DiskCategory, PrunePreviewRequest, disk_client::DiskClient, disk_server::Disk,
};
use serde_json::json;

use super::*;

const DEFAULT_GET: &str =
    "GET /v1.57/system/df?verbose=1&type=container&type=image&type=build-cache HTTP/1.1\r\n";
const VOLUME_GET: &str = "GET /v1.57/system/df?verbose=1&type=volume HTTP/1.1\r\n";

#[tokio::test]
async fn preview_is_owner_only_and_does_not_boot_a_stopped_engine() {
    let fixture = Fixture::new(Vec::new()).await;
    let unauthorized = fixture
        .api
        .prune_preview(tonic::Request::new(PrunePreviewRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(unauthorized.code(), Code::PermissionDenied);
    let mut client = DiskClient::new(fixture.channel.clone());
    let error = client
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::FailedPrecondition);
    assert_eq!(
        fixture.supervisor.snapshot().state,
        marsad::supervisor::State::Stopped
    );
}

#[tokio::test]
async fn preview_respects_existing_lifecycle_admission() {
    let fixture = ready(DEFAULT_GET, json!({})).await;
    let mut client = DiskClient::new(fixture.channel.clone());
    client
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap();
    let permit = fixture.supervisor.reserve(None).await.unwrap();
    let before = fixture.supervisor.snapshot();
    let error = client
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::Unavailable);
    assert_eq!(
        fixture.supervisor.snapshot().lifecycle_revision,
        before.lifecycle_revision
    );
    drop(permit);
}

#[tokio::test]
async fn default_preview_is_a_single_get_and_marks_missing_cache() {
    let container = "a".repeat(64);
    let image = format!("sha256:{}", "b".repeat(64));
    let body = json!({
        "ContainerUsage": {
            "TotalCount": 2,
            "Items": [
                { "Id": container, "Names": ["/stopped"], "State": "exited", "SizeRw": 17 },
                { "Id": "c".repeat(64), "Names": ["/live"], "State": "running", "SizeRw": 500 }
            ]
        },
        "ImageUsage": {
            "TotalCount": 2,
            "Items": [
                { "Id": image, "RepoTags": ["marsa-fixture:test"], "Containers": 0, "Size": 23, "SharedSize": 3 },
                { "Id": format!("sha256:{}", "d".repeat(64)), "RepoTags": null, "Containers": -1, "Size": 500, "SharedSize": 0 }
            ]
        },
        "VolumeUsage": {
            "TotalCount": 1,
            "Items": [
                { "Name": "explicit", "Driver": "local", "Scope": "local", "UsageData": { "RefCount": 0, "Size": 19 } }
            ]
        }
    });
    let fixture = ready(DEFAULT_GET, body).await;
    let mut client = DiskClient::new(fixture.channel.clone());
    let preview = client
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.candidates.len(), 2);
    assert_eq!(preview.candidates[0].id, container);
    assert_eq!(preview.candidates[0].size_bytes, Some(17));
    assert_eq!(preview.candidates[1].id, image);
    assert_eq!(preview.candidates[1].size_bytes, Some(20));
    assert_eq!(
        preview.unavailable_categories,
        [DiskCategory::BuildCache as i32]
    );
    assert_eq!(preview.estimated_reclaimable_bytes, None);
}

#[tokio::test]
async fn selected_volume_and_unknown_estimate_follow_engine_inventory() {
    let body = json!({
        "VolumeUsage": {
            "TotalCount": 3,
            "Items": [
                { "Name": "selected", "Driver": "local", "Scope": "local", "UsageData": { "RefCount": 0, "Size": -1 } },
                { "Name": "unselected", "Driver": "local", "Scope": "local", "UsageData": { "RefCount": 0, "Size": 19 } },
                { "Name": "active", "Driver": "local", "Scope": "local", "UsageData": { "RefCount": 1, "Size": 900 } }
            ]
        }
    });
    let fixture = ready(VOLUME_GET, body).await;
    let mut client = DiskClient::new(fixture.channel.clone());
    let mut request = PrunePreviewRequest {
        categories: vec![DiskCategory::Volumes as i32],
        volume_names: vec!["selected".into()],
    };
    let preview = client
        .prune_preview(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.candidates.len(), 1);
    assert_eq!(preview.candidates[0].name, "selected");
    assert_eq!(preview.candidates[0].size_bytes, None);
    assert_eq!(preview.estimated_reclaimable_bytes, None);
    request.volume_names = vec!["active".into()];
    assert_eq!(
        client.prune_preview(request).await.unwrap_err().code(),
        Code::FailedPrecondition
    );
}

#[tokio::test]
async fn positive_cache_count_without_records_is_unavailable() {
    let body = json!({
        "ContainerUsage": {},
        "ImageUsage": {},
        "BuildCacheUsage": {"TotalCount": 2}
    });
    let fixture = ready(DEFAULT_GET, body).await;
    let preview = DiskClient::new(fixture.channel.clone())
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(preview.candidates.is_empty());
    assert_eq!(
        preview.unavailable_categories,
        [DiskCategory::BuildCache as i32]
    );
    assert_eq!(preview.estimated_reclaimable_bytes, None);
}

async fn ready(target: &str, body: serde_json::Value) -> Fixture {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let fixture = Fixture::with_docker(
        vec![AgentStep::Send(b"HR".to_vec())],
        vec![
            AgentStep::Expect(target.as_bytes().to_vec()),
            AgentStep::Send(response.into_bytes()),
        ],
    )
    .await;
    fixture.supervisor.ensure_ready().await.unwrap();
    fixture
}

#[tokio::test]
async fn reported_cache_records_are_filtered_and_their_known_sizes_are_added() {
    let body = json!({
        "ContainerUsage": {},
        "ImageUsage": {},
        "BuildCacheUsage": {
            "TotalCount": 4,
            "Items": [
                { "ID": "cache-eligible", "Type": "regular", "InUse": false, "Shared": false, "Size": 11 },
                { "ID": "cache-active", "Type": "regular", "InUse": true, "Shared": false, "Size": 100 },
                { "ID": "cache-shared", "Type": "regular", "InUse": false, "Shared": true, "Size": 200 },
                { "ID": "cache-internal", "Type": "internal", "InUse": false, "Shared": false, "Size": 300 }
            ]
        }
    });
    let fixture = ready(DEFAULT_GET, body).await;
    let preview = DiskClient::new(fixture.channel.clone())
        .prune_preview(PrunePreviewRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.candidates.len(), 1);
    assert_eq!(preview.candidates[0].id, "cache-eligible");
    assert_eq!(preview.candidates[0].size_bytes, Some(11));
    assert!(preview.unavailable_categories.is_empty());
    assert_eq!(preview.estimated_reclaimable_bytes, Some(11));
}
