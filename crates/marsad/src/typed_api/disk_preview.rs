//! Report an owner-requested snapshot of unused guest storage without editing Docker state.

use std::{collections::BTreeSet, io, time::Duration};

use marsa_proto::host::v1::{
    DiskCategory, PruneCandidate, PrunePreviewRequest, PrunePreviewResponse,
};
use serde::Deserialize;
use tokio::time::timeout;
use tonic::Status;

use crate::guest_control::docker_http;

use super::TypedApi;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 1024 * 1024;
const MAX_CANDIDATES: usize = 512;
const MAX_NAMES: usize = 32;
const MAX_DISPLAY: usize = 256;
const DEFAULT: [DiskCategory; 3] = [
    DiskCategory::Containers,
    DiskCategory::Images,
    DiskCategory::BuildCache,
];

impl TypedApi {
    pub(super) async fn preview_disk_cleanup(
        &self,
        request: PrunePreviewRequest,
    ) -> Result<PrunePreviewResponse, Status> {
        let selection = Selection::new(request)?;
        let supervisor = self.supervisor.as_ref().ok_or_else(not_running)?;
        let uri = selection.uri();
        let bytes = timeout(TIMEOUT, async {
            let stream = supervisor.connect_running_docker().await?;
            docker_http::get(stream, &uri, MAX_BODY).await
        })
        .await
        .map_err(|_| Status::deadline_exceeded("Docker disk inventory timed out"))?
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotConnected => not_running(),
            _ => Status::unavailable("could not inspect Docker disk inventory"),
        })?;
        let usage = serde_json::from_slice::<DiskUsage>(&bytes)
            .map_err(|_| Status::data_loss("invalid Docker disk inventory"))?;
        usage.preview(selection)
    }
}

fn not_running() -> Status {
    Status::failed_precondition("disk cleanup preview requires a running VM")
}

struct Selection {
    categories: BTreeSet<DiskCategory>,
    volume_names: BTreeSet<String>,
}

impl Selection {
    fn new(request: PrunePreviewRequest) -> Result<Self, Status> {
        if request.categories.len() > 4 || request.volume_names.len() > MAX_NAMES {
            return Err(Status::invalid_argument(
                "too many cleanup preview selectors",
            ));
        }
        let mut categories = BTreeSet::new();
        if request.categories.is_empty() {
            categories.extend(DEFAULT);
        } else {
            for category in request.categories {
                let category = DiskCategory::try_from(category)
                    .ok()
                    .filter(|category| {
                        matches!(
                            category,
                            DiskCategory::Containers
                                | DiskCategory::Images
                                | DiskCategory::Volumes
                                | DiskCategory::BuildCache
                        )
                    })
                    .ok_or_else(|| {
                        Status::invalid_argument("unsupported cleanup preview category")
                    })?;
                if !categories.insert(category) {
                    return Err(Status::invalid_argument(
                        "duplicate cleanup preview category",
                    ));
                }
            }
        }
        let mut volume_names = BTreeSet::new();
        for name in request.volume_names {
            if !volume_name(&name) || !volume_names.insert(name) {
                return Err(Status::invalid_argument("invalid or duplicate volume name"));
            }
        }
        if !volume_names.is_empty() {
            categories.insert(DiskCategory::Volumes);
        }
        Ok(Self {
            categories,
            volume_names,
        })
    }

    fn uri(&self) -> String {
        let mut uri = "/v1.57/system/df?verbose=1".to_owned();
        for category in &self.categories {
            uri.push_str(match category {
                DiskCategory::Containers => "&type=container",
                DiskCategory::Images => "&type=image",
                DiskCategory::Volumes => "&type=volume",
                DiskCategory::BuildCache => "&type=build-cache",
                _ => "",
            });
        }
        uri
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskUsage {
    container_usage: Option<Rows<Container>>,
    image_usage: Option<Rows<Image>>,
    volume_usage: Option<Rows<Volume>>,
    build_cache_usage: Option<Rows<Cache>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Rows<T> {
    items: Option<Vec<T>>,
    #[serde(default)]
    total_count: i64,
}

impl<T> Rows<T> {
    fn complete(&self) -> Option<&[T]> {
        let items = self.items.as_deref().unwrap_or_default();
        (usize::try_from(self.total_count).ok() == Some(items.len())).then_some(items)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Container {
    id: String,
    names: Option<Vec<String>>,
    state: String,
    size_rw: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Image {
    id: String,
    repo_tags: Option<Vec<String>>,
    containers: i64,
    size: i64,
    shared_size: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Volume {
    name: String,
    driver: String,
    scope: String,
    usage_data: Option<VolumeUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeUsage {
    ref_count: i64,
    size: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Cache {
    #[serde(rename = "ID")]
    id: String,
    in_use: bool,
    shared: bool,
    #[serde(rename = "Type")]
    kind: String,
    size: i64,
}

impl DiskUsage {
    fn preview(self, selection: Selection) -> Result<PrunePreviewResponse, Status> {
        let mut preview = PrunePreviewResponse::default();
        for category in selection.categories {
            match category {
                DiskCategory::Containers => add(
                    &mut preview,
                    category,
                    self.container_usage.as_ref(),
                    |item| {
                        matches!(item.state.as_str(), "created" | "exited" | "dead").then(|| {
                            let name = item
                                .names
                                .as_ref()
                                .and_then(|names| names.first())
                                .map(|name| name.trim_start_matches('/'))
                                .unwrap_or(&item.id);
                            (item.id.as_str(), name, item.size_rw.and_then(nonnegative))
                        })
                    },
                )?,
                DiskCategory::Images => {
                    add(&mut preview, category, self.image_usage.as_ref(), |item| {
                        (item.containers == 0).then(|| {
                            let name = item
                                .repo_tags
                                .as_ref()
                                .and_then(|tags| tags.first())
                                .map_or(item.id.as_str(), String::as_str);
                            let size = nonnegative(item.size)
                                .zip(nonnegative(item.shared_size))
                                .and_then(|(size, shared)| size.checked_sub(shared));
                            (item.id.as_str(), name, size)
                        })
                    })?
                }
                DiskCategory::Volumes => {
                    add(&mut preview, category, self.volume_usage.as_ref(), |item| {
                        let usage = item.usage_data.as_ref()?;
                        (item.driver == "local"
                            && item.scope == "local"
                            && usage.ref_count == 0
                            && (selection.volume_names.is_empty()
                                || selection.volume_names.contains(&item.name)))
                        .then(|| {
                            (
                                item.name.as_str(),
                                item.name.as_str(),
                                nonnegative(usage.size),
                            )
                        })
                    })?
                }
                DiskCategory::BuildCache => add(
                    &mut preview,
                    category,
                    self.build_cache_usage.as_ref(),
                    |item| {
                        (!item.in_use
                            && !item.shared
                            && !matches!(item.kind.as_str(), "internal" | "frontend"))
                        .then(|| (item.id.as_str(), item.id.as_str(), nonnegative(item.size)))
                    },
                )?,
                _ => (),
            }
        }
        if !selection.volume_names.is_empty()
            && !preview
                .unavailable_categories
                .contains(&(DiskCategory::Volumes as i32))
            && selection.volume_names.iter().any(|name| {
                !preview
                    .candidates
                    .iter()
                    .any(|row| row.category() == DiskCategory::Volumes && &row.id == name)
            })
        {
            return Err(Status::failed_precondition(
                "a named volume is missing or in use",
            ));
        }
        preview
            .candidates
            .sort_by(|a, b| (a.category, &a.id).cmp(&(b.category, &b.id)));
        if preview.unavailable_categories.is_empty() {
            // A missing size or overflow is reported as an unknown total.
            preview.estimated_reclaimable_bytes = preview
                .candidates
                .iter()
                .try_fold(0_u64, |sum, row| sum.checked_add(row.size_bytes?));
        }
        Ok(preview)
    }
}

fn add<'a, T>(
    preview: &mut PrunePreviewResponse,
    category: DiskCategory,
    rows: Option<&'a Rows<T>>,
    candidate: impl Fn(&'a T) -> Option<(&'a str, &'a str, Option<u64>)>,
) -> Result<(), Status> {
    let items = rows.and_then(Rows::complete);
    let Some(items) = items else {
        preview.unavailable_categories.push(category as i32);
        return Ok(());
    };
    let mut ids = BTreeSet::new();
    for (id, name, bytes) in items.iter().filter_map(candidate) {
        if !candidate_id(category, id)
            || name.is_empty()
            || name.len() > MAX_DISPLAY
            || !name.bytes().all(|byte| byte.is_ascii_graphic())
            || !ids.insert(id)
        {
            return Err(Status::data_loss("invalid Docker cleanup identity"));
        }
        if preview.candidates.len() == MAX_CANDIDATES {
            return Err(Status::resource_exhausted(
                "too many Docker cleanup candidates",
            ));
        }
        preview.candidates.push(PruneCandidate {
            category: category as i32,
            id: id.to_owned(),
            name: name.to_owned(),
            size_bytes: bytes,
        });
    }
    Ok(())
}

fn candidate_id(category: DiskCategory, id: &str) -> bool {
    match category {
        DiskCategory::Containers => full_digest(id),
        DiskCategory::Images => id.strip_prefix("sha256:").is_some_and(full_digest),
        DiskCategory::Volumes => volume_name(id),
        DiskCategory::BuildCache => safe_identifier(id),
        _ => false,
    }
}

fn full_digest(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn volume_name(name: &str) -> bool {
    safe_identifier(name)
        && !name.contains(':')
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn safe_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_DISPLAY
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b":_.-".contains(&byte))
}

fn nonnegative(bytes: i64) -> Option<u64> {
    u64::try_from(bytes).ok()
}
