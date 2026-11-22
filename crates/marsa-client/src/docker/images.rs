//! Stream public image pulls and launch existing local image IDs.

use hyper::Method;
use serde::Deserialize;
use serde_json::json;

use super::{
    ContainerAction, ContainerId, DockerClient, DockerError, ImageId, ResourceName,
    stream::ResponseBody,
};

/// A user-supplied reference to a public registry image.
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

impl DockerClient {
    /// Start an anonymous public-registry pull for the guest's Linux/ARM64 platform.
    pub async fn pull_image(&self, image: &ImageReference) -> Result<ImagePull, DockerError> {
        let path = format!(
            "/images/create?fromImage={}&platform=linux%2Farm64",
            image.query()
        );
        let (_, source) = self.request(Method::POST, &path).await?;
        Ok(ImagePull {
            source,
            line: Vec::new(),
        })
    }

    /// Create a local container from an existing image ID and start it with image defaults.
    ///
    /// If starting fails, the result identifies the created container for a later retry.
    pub async fn run_image(
        &self,
        image: &ImageId,
        name: Option<&ResourceName>,
    ) -> Result<ImageRun, DockerError> {
        let path = name.map_or_else(
            || "/containers/create".to_owned(),
            |name| format!("/containers/create?name={}", name.as_str()),
        );
        let created: NewContainer = self
            .post_json(&path, json!({"Image": image.as_str()}))
            .await?;
        match self.act(&created.id, ContainerAction::Start).await {
            Ok(()) => Ok(ImageRun::Started(created.id)),
            Err(start_error) => Ok(ImageRun::Created {
                id: created.id,
                start_error,
            }),
        }
    }
}
