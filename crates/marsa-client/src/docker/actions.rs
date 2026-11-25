//! Fixed routes for desktop image, volume and network actions.

use std::collections::BTreeMap;

use hyper::Method;
use serde::{Deserialize, Deserializer};
use serde_json::json;

use super::{DockerClient, DockerError, RemovalMode, VolumeSummary, null_to_default};

/// A Docker-compatible volume or network name for fixed HTTP routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceName(String);

impl ResourceName {
    /// Validate and retain a name without accepting route separators or query characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, DockerError> {
        let value = value.into();
        if !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            || value.len() > 255
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(DockerError::InvalidResourceName);
        }
        Ok(Self(value))
    }

    /// The validated name as received from the owner or engine.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Volume removal authorized by reproducing its exact name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRemoval(ResourceName);

impl VolumeRemoval {
    /// Accept only the exact name typed by the owner in the confirmation view.
    pub fn confirm(name: ResourceName, typed: &str) -> Result<Self, DockerError> {
        if name.as_str() != typed {
            return Err(DockerError::VolumeConfirmation);
        }
        Ok(Self(name))
    }
}

/// Network removal authorized by the owner after reviewing the selected network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRemoval(String);

impl NetworkRemoval {
    /// Validate the ID associated with the confirmed network.
    pub fn confirmed(id: impl Into<String>) -> Result<Self, DockerError> {
        Ok(Self(full_hex(id.into())?))
    }
}

/// Image removal authorized for one explicitly identified image and force mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRemoval {
    id: ImageId,
    mode: RemovalMode,
}

impl ImageRemoval {
    /// Record the image and mode confirmed by its owner.
    pub fn confirmed(id: ImageId, mode: RemovalMode) -> Self {
        Self { id, mode }
    }
}

/// Canonical content-addressed image identity used by inspect and remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageId(String);

impl ImageId {
    /// Accept a Docker image ID with the SHA-256 prefix and its full digest.
    pub fn parse(value: impl Into<String>) -> Result<Self, DockerError> {
        let value = value.into();
        let digest = value
            .strip_prefix("sha256:")
            .ok_or(DockerError::InvalidResourceName)?;
        validate_hex(digest)?;
        Ok(Self(value))
    }

    /// Return the complete content identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_hex(value: &str) -> Result<(), DockerError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(DockerError::InvalidResourceName);
    }
    Ok(())
}

fn full_hex(value: String) -> Result<String, DockerError> {
    validate_hex(&value)?;
    Ok(value)
}

/// Non-sensitive image inspection fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageDetails {
    /// Docker image ID returned by inspect.
    pub id: String,
    /// Repository tags, which can be empty.
    #[serde(default, deserialize_with = "null_to_default")]
    pub repo_tags: Vec<String>,
    /// Digest references associated with this image.
    #[serde(default, deserialize_with = "null_to_default")]
    pub repo_digests: Vec<String>,
    /// Image creation time in its original format.
    pub created: Option<String>,
    /// Target CPU architecture.
    pub architecture: String,
    /// Target operating system.
    pub os: String,
    /// Total reported image size in bytes.
    pub size: u64,
    /// Optional author string from the image metadata.
    #[serde(default)]
    pub author: Option<String>,
    /// Container configuration baked into the image.
    #[serde(default)]
    pub config: Option<ImageConfig>,
    /// Root filesystem layer descriptors.
    #[serde(default, rename = "RootFS")]
    pub root_fs: Option<ImageRootFs>,
}

/// Image `Config` fields used by the details modal.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ImageConfig {
    /// Default command.
    #[serde(default, deserialize_with = "null_to_default")]
    pub cmd: Vec<String>,
    /// Default entrypoint.
    #[serde(default, deserialize_with = "null_to_default")]
    pub entrypoint: Vec<String>,
    /// Environment assignments (`KEY=value`).
    #[serde(default, deserialize_with = "null_to_default")]
    pub env: Vec<String>,
    /// Image labels.
    #[serde(default, deserialize_with = "null_to_default")]
    pub labels: BTreeMap<String, String>,
    /// Working directory inside the container.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Default user.
    #[serde(default)]
    pub user: Option<String>,
    /// Exposed port specs (`80/tcp`, …).
    #[serde(default, deserialize_with = "exposed_port_keys")]
    pub exposed_ports: Vec<String>,
}

/// RootFS section from image inspect.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ImageRootFs {
    /// Layer digests reported by the engine.
    #[serde(default, deserialize_with = "null_to_default")]
    pub layers: Vec<String>,
}

fn exposed_port_keys<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: Option<BTreeMap<String, serde::de::IgnoredAny>> = Option::deserialize(deserializer)?;
    Ok(map.unwrap_or_default().into_keys().collect())
}

/// Identity of a newly created Docker network.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedNetwork {
    /// Identifier assigned by the engine.
    #[serde(rename = "Id")]
    pub id: String,
}

/// One layer from `GET /images/{name}/history`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageHistoryLayer {
    /// Layer or image content ID.
    pub id: String,
    /// Unix creation timestamp.
    pub created: i64,
    /// Dockerfile instruction or build step that produced the layer.
    pub created_by: String,
    /// Tags pointing at this layer, when any.
    #[serde(default, deserialize_with = "null_to_default")]
    pub tags: Vec<String>,
    /// Incremental size contributed by this layer, in bytes.
    pub size: i64,
    /// Optional engine comment (for example BuildKit metadata).
    #[serde(default)]
    pub comment: String,
}

impl DockerClient {
    pub(crate) async fn resolve_compose_secret_image(
        &self,
        image: super::ImageReference,
    ) -> Result<ImageId, DockerError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Inspect {
            id: String,
            config: Config,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Config {
            volumes: Option<BTreeMap<String, serde::de::IgnoredAny>>,
        }

        let inspection: Inspect = self
            .snapshot(&format!("/images/{}/json", image.query()))
            .await?;
        if inspection
            .config
            .volumes
            .is_some_and(|volumes| !volumes.is_empty())
        {
            return Err(DockerError::ResourceIdentity);
        }
        ImageId::parse(inspection.id)
    }

    /// Read image metadata by its full content identifier.
    pub async fn inspect_image(&self, id: &ImageId) -> Result<ImageDetails, DockerError> {
        let details: ImageDetails = self
            .snapshot(&format!("/images/{}/json", id.as_str()))
            .await?;
        if details.id != id.as_str() {
            return Err(DockerError::ResourceIdentity);
        }
        Ok(details)
    }

    /// Read the build history / layer stack for an image.
    pub async fn image_history(&self, id: &ImageId) -> Result<Vec<ImageHistoryLayer>, DockerError> {
        self.snapshot(&format!("/images/{}/history", id.as_str()))
            .await
    }

    /// Create a bridge network on the local engine.
    pub async fn create_network(&self, name: &ResourceName) -> Result<CreatedNetwork, DockerError> {
        self.post_json(
            "/networks/create",
            json!({"Name": name.as_str(), "Driver": "bridge"}),
        )
        .await
    }

    /// Create a volume with Docker's local driver.
    pub async fn create_volume(&self, name: &ResourceName) -> Result<VolumeSummary, DockerError> {
        self.post_json(
            "/volumes/create",
            json!({"Name": name.as_str(), "Driver": "local"}),
        )
        .await
    }

    /// Remove a volume after exact-name confirmation, without forcing removal.
    pub async fn remove_volume(&self, removal: VolumeRemoval) -> Result<(), DockerError> {
        let path = format!("/volumes/{}?force=false", removal.0.as_str());
        self.request(Method::DELETE, &path).await?;
        Ok(())
    }

    /// Remove the confirmed network. Docker rejects networks with active endpoints.
    pub async fn remove_network(&self, removal: NetworkRemoval) -> Result<(), DockerError> {
        self.request(Method::DELETE, &format!("/networks/{}", removal.0))
            .await?;
        Ok(())
    }

    /// Remove the confirmed image. Force mode requires its own explicit choice.
    pub async fn remove_image(&self, removal: ImageRemoval) -> Result<(), DockerError> {
        let path = format!(
            "/images/{}?force={}",
            removal.id.as_str(),
            removal.mode == RemovalMode::Force,
        );
        self.request(Method::DELETE, &path).await?;
        Ok(())
    }
}
