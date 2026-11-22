//! Normalize daemon and Docker snapshots and notify views through typed selectors.

use std::{collections::BTreeMap, sync::Arc};

use marsa_client::{
    api::{DaemonEvent, DaemonSnapshot, EngineStatus, daemon_event::Body},
    docker::{ContainerId, ContainerSummary, ImageSummary, NetworkSummary, VolumeSummary},
    runtime::{Latest, Observed},
};

/// The newest daemon snapshot and Docker resources, indexed by stable IDs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppState {
    daemon: Option<DaemonSnapshot>,
    containers: Arc<BTreeMap<ContainerId, ContainerSummary>>,
    images: Arc<BTreeMap<String, ImageSummary>>,
    volumes: Arc<BTreeMap<String, VolumeSummary>>,
    networks: Arc<BTreeMap<String, NetworkSummary>>,
    compose_projects: Arc<BTreeMap<String, ComposeProject>>,
}

#[derive(Default)]
pub(crate) struct ResourceSnapshot {
    pub(crate) containers: Vec<ContainerSummary>,
    pub(crate) images: Vec<ImageSummary>,
    pub(crate) volumes: Vec<VolumeSummary>,
    pub(crate) networks: Vec<NetworkSummary>,
}

/// A Compose project discovered from the labels of existing containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeProject {
    /// Project name reported by Docker Compose.
    pub name: String,
    /// Full identifiers of the containers carrying this project label.
    pub containers: Vec<ContainerId>,
}

impl AppState {
    pub(crate) fn index_resources(snapshot: ResourceSnapshot) -> Self {
        let containers = container_index(snapshot.containers);
        let projects = project_index(&containers);
        let images = image_index(snapshot.images);
        let volumes = Arc::new(
            snapshot
                .volumes
                .into_iter()
                .map(|row| (row.name.clone(), row))
                .collect(),
        );
        let networks = Arc::new(
            snapshot
                .networks
                .into_iter()
                .map(|row| (row.id.clone(), row))
                .collect(),
        );
        Self {
            daemon: None,
            containers,
            compose_projects: projects,
            images,
            volumes,
            networks,
        }
    }

    /// Last observed daemon state.
    pub fn daemon(&self) -> Option<&DaemonSnapshot> {
        self.daemon.as_ref()
    }

    /// Last observed engine lifecycle state.
    pub fn engine(&self) -> Option<&EngineStatus> {
        self.daemon()?.engine.as_ref()
    }

    /// Find a container row by its full Docker ID.
    pub fn container(&self, id: &ContainerId) -> Option<&ContainerSummary> {
        self.containers.get(id)
    }

    /// Read the current normalized container collection.
    pub fn containers(&self) -> Arc<BTreeMap<ContainerId, ContainerSummary>> {
        Arc::clone(&self.containers)
    }

    /// Find an image by its engine identifier.
    pub fn image(&self, id: &str) -> Option<&ImageSummary> {
        self.images.get(id)
    }

    /// Read the current normalized image collection.
    pub fn images(&self) -> Arc<BTreeMap<String, ImageSummary>> {
        Arc::clone(&self.images)
    }

    /// Volumes indexed by their canonical names.
    pub fn volumes(&self) -> Arc<BTreeMap<String, VolumeSummary>> {
        Arc::clone(&self.volumes)
    }

    /// Networks indexed by their complete IDs.
    pub fn networks(&self) -> Arc<BTreeMap<String, NetworkSummary>> {
        Arc::clone(&self.networks)
    }

    /// Existing Compose projects and their container identifiers.
    pub fn compose_projects(&self) -> Arc<BTreeMap<String, ComposeProject>> {
        Arc::clone(&self.compose_projects)
    }
}

/// Coalesced application state shared between services and views.
#[derive(Clone)]
pub struct Store(Latest<AppState>);

impl Default for Store {
    fn default() -> Self {
        Self(Latest::new(AppState::default()))
    }
}

impl Store {
    /// Watch a projection. Changes outside that projection never deliver a new value.
    pub fn select<T: PartialEq, F: Fn(&AppState) -> T>(&self, selector: F) -> Selection<T, F> {
        let mut observed = self.0.observe();
        let value = observed.read(&selector);
        Selection {
            observed,
            selector,
            value,
        }
    }

    pub(crate) fn apply(&self, event: DaemonEvent) {
        let Some(body) = event.body else { return };
        self.0.update(|state| {
            if let Body::Snapshot(snapshot) = body {
                return replace(&mut state.daemon, Some(snapshot));
            }
            let Some(daemon) = &mut state.daemon else {
                return false;
            };
            match body {
                Body::Snapshot(_) => false,
                Body::Engine(engine) => replace(&mut daemon.engine, Some(engine)),
                Body::Settings(settings) => {
                    replace(&mut daemon.settings_revision, settings.revision)
                }
                Body::Shares(shares) => replace(&mut daemon.shares, Some(shares)),
                Body::Network(network) => replace(&mut daemon.network, Some(network)),
                Body::ForwardedPorts(ports) => replace(&mut daemon.forwarded_ports, Some(ports)),
                Body::Update(update) => replace(&mut daemon.update, Some(update)),
                Body::Approvals(approvals) => replace(&mut daemon.approvals, Some(approvals)),
                Body::Grants(grants) => replace(&mut daemon.grants, Some(grants)),
                Body::BindApprovals(approvals) => {
                    replace(&mut daemon.bind_approvals, Some(approvals))
                }
            }
        });
    }

    pub(crate) fn replace_docker(&self, snapshot: ResourceSnapshot) {
        let AppState {
            containers,
            compose_projects: projects,
            images,
            volumes,
            networks,
            ..
        } = AppState::index_resources(snapshot);
        self.0.update(|state| {
            let containers_changed = replace(&mut state.containers, containers);
            let projects_changed = replace(&mut state.compose_projects, projects);
            let images_changed = replace(&mut state.images, images);
            let volumes_changed = replace(&mut state.volumes, volumes);
            let networks_changed = replace(&mut state.networks, networks);
            containers_changed
                || projects_changed
                || images_changed
                || volumes_changed
                || networks_changed
        });
    }

    pub(crate) fn reset(&self) {
        self.0.update(|state| {
            if state.daemon.is_none()
                && state.containers.is_empty()
                && state.images.is_empty()
                && state.volumes.is_empty()
                && state.networks.is_empty()
            {
                return false;
            }
            *state = AppState::default();
            true
        });
    }
}

fn container_index(rows: Vec<ContainerSummary>) -> Arc<BTreeMap<ContainerId, ContainerSummary>> {
    Arc::new(rows.into_iter().map(|row| (row.id.clone(), row)).collect())
}

fn image_index(rows: Vec<ImageSummary>) -> Arc<BTreeMap<String, ImageSummary>> {
    Arc::new(rows.into_iter().map(|row| (row.id.clone(), row)).collect())
}

fn project_index(
    containers: &BTreeMap<ContainerId, ContainerSummary>,
) -> Arc<BTreeMap<String, ComposeProject>> {
    let mut projects = BTreeMap::new();
    for container in containers.values() {
        if let Some(name) = container.compose_project() {
            projects
                .entry(name.to_owned())
                .or_insert_with(|| ComposeProject {
                    name: name.to_owned(),
                    containers: Vec::new(),
                })
                .containers
                .push(container.id.clone());
        }
    }
    Arc::new(projects)
}

fn replace<T: PartialEq>(current: &mut T, value: T) -> bool {
    if *current == value {
        return false;
    }
    *current = value;
    true
}

/// A view's retained selection and change cursor.
pub struct Selection<T, F> {
    observed: Observed<AppState>,
    selector: F,
    value: T,
}

impl<T: PartialEq, F: Fn(&AppState) -> T> Selection<T, F> {
    /// Read the most recently delivered projection.
    pub fn current(&self) -> &T {
        &self.value
    }

    /// Wait until this projection changes or the store closes.
    pub async fn changed(&mut self) -> Option<&T> {
        while self.observed.changed().await {
            let value = self.observed.read(&self.selector);
            if replace(&mut self.value, value) {
                return Some(&self.value);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_deliver_only_selected_value_changes() {
        let runtime = marsa_client::runtime::Background::new().unwrap();
        let store = Store::default();
        let mut receiver = store.select(|state| state.engine().map(|engine| engine.state));
        let (send, recv) = std::sync::mpsc::channel();
        let first = send.clone();
        let _observer = runtime.spawn(async move {
            first.send(*receiver.current()).unwrap();
            while let Some(&state) = receiver.changed().await {
                if send.send(state).is_err() {
                    break;
                }
            }
        });
        let timeout = std::time::Duration::from_secs(2);
        assert_eq!(recv.recv_timeout(timeout).unwrap(), None);
        store.apply(DaemonEvent {
            body: Some(Body::Snapshot(DaemonSnapshot {
                engine: Some(EngineStatus {
                    state: 2,
                    ..Default::default()
                }),
                ..Default::default()
            })),
            ..Default::default()
        });
        assert_eq!(recv.recv_timeout(timeout).unwrap(), Some(2));
        store.apply(DaemonEvent {
            body: Some(Body::Settings(marsa_client::api::SettingsChanged {
                revision: 1,
            })),
            ..Default::default()
        });
        assert!(
            recv.recv_timeout(std::time::Duration::from_millis(30))
                .is_err()
        );
        store.apply(DaemonEvent {
            body: Some(Body::Engine(EngineStatus {
                state: 3,
                ..Default::default()
            })),
            ..Default::default()
        });
        assert_eq!(recv.recv_timeout(timeout).unwrap(), Some(3));
    }
}
