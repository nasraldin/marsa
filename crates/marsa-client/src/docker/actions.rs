//! Fixed routes for desktop image, volume and network actions.

use hyper::Method;
use serde::Deserialize;
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
}

/// Identity of a newly created Docker network.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedNetwork {
    /// Identifier assigned by the engine.
    #[serde(rename = "Id")]
    pub id: String,
}

impl DockerClient {
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
