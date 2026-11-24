//! Stream public image pulls and launch existing local image IDs.

use std::collections::BTreeMap;

use hyper::Method;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{
    ContainerAction, ContainerId, DockerClient, DockerError, DockerScope, ImageId, ResourceName,
    container_config::{
        ConfigMountKind, ConfigNetwork, ConfigPortProtocol, ConfigRestartPolicy,
        ContainerCreateOptions,
    },
    stream::ResponseBody,
};

/// A user-supplied reference to an image in a registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReference(String);

impl ImageReference {
    /// Validate reference characters before placing the value in a request query.
    ///
    /// Docker validates the repository, tag and digest grammar.
    pub fn parse(value: impl Into<String>) -> Result<Self, DockerError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 512
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/' | b'@' | b'[' | b']')
            })
        {
            return Err(DockerError::InvalidResourceName);
        }
        Ok(Self(value))
    }

    /// Return the validated reference as typed by the caller.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn query(&self) -> String {
        use std::fmt::Write as _;

        let mut query = String::new();
        for byte in self.0.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
                query.push(char::from(byte));
            } else {
                // Writing to String cannot fail.
                let _ = write!(query, "%{byte:02X}");
            }
        }
        query
    }
}

/// One engine progress record from a public image pull.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PullProgress {
    /// Layer identifier, when this update belongs to one layer.
    pub id: Option<String>,
    /// Descriptive status supplied by Docker.
    pub status: Option<String>,
    /// Byte counters when this layer has a transfer in progress.
    #[serde(rename = "progressDetail")]
    pub progress: Option<PullCounters>,
}

/// Byte counters in an image pull progress record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PullCounters {
    /// Bytes transferred.
    pub current: Option<u64>,
    /// Total bytes expected.
    pub total: Option<u64>,
}

#[derive(Deserialize)]
struct PullWire {
    #[serde(flatten)]
    progress: PullProgress,
    error: Option<String>,
    #[serde(rename = "errorDetail")]
    error_detail: Option<serde::de::IgnoredAny>,
}

/// A cancellable stream of engine progress for an image pull.
pub struct ImagePull {
    source: ResponseBody,
    line: Vec<u8>,
}

impl ImagePull {
    /// Read the next progress record or the terminal engine error.
    pub async fn next(&mut self) -> Result<Option<PullProgress>, DockerError> {
        let Some(event) = self.source.json::<PullWire>(&mut self.line).await? else {
            return Ok(None);
        };
        if let Some(message) = event.error {
            if message.contains("no matching manifest for linux/arm64") {
                return Err(DockerError::ImagePlatform);
            }
            return Err(DockerError::ImagePull);
        }
        if event.error_detail.is_some() {
            return Err(DockerError::ImagePull);
        }
        Ok(Some(event.progress))
    }
}

#[derive(Deserialize)]
struct NewContainer {
    #[serde(rename = "Id")]
    id: ContainerId,
}

/// Outcome of creating and starting a container from an existing local image.
#[derive(Debug)]
pub enum ImageRun {
    /// The new container started.
    Started(ContainerId),
    /// Creation succeeded, but the engine rejected starting it.
    Created {
        /// Identifier retained in Docker for retry or explicit removal.
        id: ContainerId,
        /// The error from the start request.
        start_error: DockerError,
    },
}

/// Whether the user reviewed these host bind mounts for the pinned external engine.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ContainerMountReview {
    /// No external host mount review was given.
    #[default]
    Unreviewed,
    /// The user reviewed the selected external engine and the exact host mount options.
    ApprovedExternalHostPaths,
}

pub(super) fn valid_tag(tag: &str) -> bool {
    (1..=128).contains(&tag.len())
        && tag
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

impl DockerClient {
    /// Start an anonymous public-registry pull using the selected engine's platform.
    pub async fn pull_image(&self, image: &ImageReference) -> Result<ImagePull, DockerError> {
        let mut path = format!("/images/create?fromImage={}", image.query());
        if matches!(self.scope, super::DockerScope::Marsa(_)) {
            path.push_str("&platform=linux%2Farm64");
        }
        let (_, source) = self.request(Method::POST, &path).await?;
        Ok(ImagePull {
            source,
            line: Vec::new(),
        })
    }

    /// Add the requested repository and tag to an existing image on this engine.
    pub async fn tag_image(&self, image: &ImageId, destination: &str) -> Result<(), DockerError> {
        let (repository, tag) = destination
            .rsplit_once(':')
            .ok_or(DockerError::InvalidResourceName)?;
        if repository.contains('@') || !valid_tag(tag) {
            return Err(DockerError::InvalidResourceName);
        }
        let repository = ImageReference::parse(repository)?;
        let path = format!(
            "/images/{}/tag?repo={}&tag={tag}",
            image.as_str(),
            repository.query()
        );
        self.request(Method::POST, &path).await?;
        Ok(())
    }

    /// Create a local container from an existing image ID and start it with image defaults.
    ///
    /// If starting fails, the result identifies the created container for a later retry.
    pub async fn run_image(
        &self,
        image: &ImageId,
        name: Option<&ResourceName>,
    ) -> Result<ImageRun, DockerError> {
        self.create_and_start(name, json!({"Image": image.as_str()}))
            .await
    }

    /// Create and start a container with reviewed typed options.
    ///
    /// Marsa's own bind mount allowlist is enforced by the engine's authorization path.
    /// External engines require an explicit review of host bind sources before submission.
    pub async fn run_image_configured(
        &self,
        image: &ImageId,
        name: Option<&ResourceName>,
        options: &ContainerCreateOptions,
        review: ContainerMountReview,
    ) -> Result<ImageRun, DockerError> {
        options.validate()?;
        if matches!(self.scope, DockerScope::External { .. })
            && options.has_host_mounts()
            && review != ContainerMountReview::ApprovedExternalHostPaths
        {
            return Err(DockerError::ExternalBindReviewRequired);
        }
        self.create_and_start(name, configured_payload(image, options))
            .await
    }

    async fn create_and_start(
        &self,
        name: Option<&ResourceName>,
        payload: Value,
    ) -> Result<ImageRun, DockerError> {
        let path = name.map_or_else(
            || "/containers/create".to_owned(),
            |name| format!("/containers/create?name={}", name.as_str()),
        );
        let created: NewContainer = self.post_json(&path, payload).await?;
        match self.act(&created.id, ContainerAction::Start).await {
            Ok(()) => Ok(ImageRun::Started(created.id)),
            Err(start_error) => Ok(ImageRun::Created {
                id: created.id,
                start_error,
            }),
        }
    }
}

fn configured_payload(image: &ImageId, options: &ContainerCreateOptions) -> Value {
    let mut payload = Map::from_iter([("Image".into(), json!(image.as_str()))]);
    let mut host = Map::new();
    let network = match options.network {
        ConfigNetwork::Default => None,
        ConfigNetwork::Bridge => Some("bridge"),
        ConfigNetwork::Disabled => Some("none"),
    };
    if let Some(network) = network {
        host.insert("NetworkMode".into(), json!(network));
    }
    let restart = match options.restart {
        ConfigRestartPolicy::Never => None,
        ConfigRestartPolicy::OnFailure => Some("on-failure"),
        ConfigRestartPolicy::Always => Some("always"),
        ConfigRestartPolicy::UnlessStopped => Some("unless-stopped"),
    };
    if let Some(restart) = restart {
        host.insert("RestartPolicy".into(), json!({"Name": restart}));
    }
    if !options.env.is_empty() {
        payload.insert(
            "Env".into(),
            Value::Array(
                options
                    .env
                    .iter()
                    .map(|pair| json!(format!("{}={}", pair.key, pair.value)))
                    .collect(),
            ),
        );
    }
    if !options.labels.is_empty() {
        payload.insert(
            "Labels".into(),
            Value::Object(
                options
                    .labels
                    .iter()
                    .map(|pair| (pair.key.clone(), json!(pair.value)))
                    .collect(),
            ),
        );
    }
    if !options.ports.is_empty() {
        let mut bindings = BTreeMap::<String, Vec<Value>>::new();
        for port in &options.ports {
            let protocol = match port.protocol {
                ConfigPortProtocol::Tcp => "tcp",
                ConfigPortProtocol::Udp => "udp",
            };
            bindings
                .entry(format!("{}/{protocol}", port.container))
                .or_default()
                .push(json!({
                    "HostIp": "127.0.0.1",
                    "HostPort": port.host.to_string(),
                }));
        }
        let exposed = bindings
            .keys()
            .map(|port| (port.clone(), json!({})))
            .collect();
        payload.insert("ExposedPorts".into(), Value::Object(exposed));
        host.insert("PortBindings".into(), json!(bindings));
    }
    if !options.mounts.is_empty() {
        host.insert("Mounts".into(), Value::Array(options.mounts.iter().map(|mount| {
            let kind = match mount.kind {
                ConfigMountKind::Host => "bind",
                ConfigMountKind::Volume => "volume",
            };
            json!({"Type": kind, "Source": mount.source, "Target": mount.target, "ReadOnly": mount.read_only})
        }).collect()));
    }
    if !host.is_empty() {
        payload.insert("HostConfig".into(), Value::Object(host));
    }
    Value::Object(payload)
}

#[cfg(test)]
mod configured_tests {
    use super::*;
    use crate::docker::container_config::{ConfigKeyValue, ConfigMount, ConfigPort};

    #[tokio::test]
    async fn external_bind_is_rejected_before_opening_an_unreviewed_engine_connection() {
        use crate::docker::{ExternalEndpoint, inventory::ExternalTransport};

        let endpoint =
            ExternalEndpoint::managed("managed:draft".to_owned(), ExternalTransport::Unavailable);
        let docker = endpoint.management_client();
        let image = ImageId::parse("sha256:".to_owned() + &"a".repeat(64)).unwrap();
        let options = ContainerCreateOptions {
            mounts: vec![ConfigMount {
                kind: ConfigMountKind::Host,
                source: "/home/engine/project".into(),
                target: "/data".into(),
                read_only: true,
            }],
            ..Default::default()
        };
        assert!(matches!(
            docker
                .run_image_configured(&image, None, &options, ContainerMountReview::Unreviewed)
                .await,
            Err(DockerError::ExternalBindReviewRequired)
        ));
        assert!(matches!(
            docker
                .run_image_configured(
                    &image,
                    None,
                    &options,
                    ContainerMountReview::ApprovedExternalHostPaths
                )
                .await,
            Err(DockerError::Socket(_))
        ));
    }

    #[test]
    fn configured_container_serializes_only_the_reviewed_engine_settings() {
        let image = ImageId::parse("sha256:".to_owned() + &"a".repeat(64)).unwrap();
        let options = ContainerCreateOptions {
            network: ConfigNetwork::Bridge,
            restart: ConfigRestartPolicy::UnlessStopped,
            env: vec![ConfigKeyValue {
                key: "MODE".to_owned(),
                value: "test=one".to_owned(),
            }],
            labels: vec![ConfigKeyValue {
                key: "org.example.group".to_owned(),
                value: "web".to_owned(),
            }],
            ports: vec![ConfigPort {
                host: 8080,
                container: 80,
                protocol: ConfigPortProtocol::Tcp,
            }],
            mounts: vec![
                ConfigMount {
                    kind: ConfigMountKind::Host,
                    source: "/Users/example/web".into(),
                    target: "/src".into(),
                    read_only: true,
                },
                ConfigMount {
                    kind: ConfigMountKind::Volume,
                    source: "web_data".into(),
                    target: "/data".into(),
                    read_only: false,
                },
            ],
        };
        assert_eq!(
            configured_payload(&image, &options),
            json!({
                "Image": image.as_str(),
                "Env": ["MODE=test=one"],
                "Labels": {"org.example.group": "web"},
                "ExposedPorts": {"80/tcp": {}},
                "HostConfig": {
                    "NetworkMode": "bridge",
                    "RestartPolicy": {"Name": "unless-stopped"},
                    "PortBindings": {"80/tcp": [{"HostIp": "127.0.0.1", "HostPort": "8080"}]},
                    "Mounts": [
                        {"Type": "bind", "Source": "/Users/example/web", "Target": "/src", "ReadOnly": true},
                        {"Type": "volume", "Source": "web_data", "Target": "/data", "ReadOnly": false}
                    ]
                },
            })
        );
    }
}
