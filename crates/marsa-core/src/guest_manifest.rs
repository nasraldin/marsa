//! The bounded metadata format for identifying and verifying a bundled guest.

use std::io::{self, Read};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const SCHEMA_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
const KERNEL: &str = "Image";
const ROOTFS: &str = "rootfs.img";

/// Versioned metadata for the arm64 Linux boot image and its read-only root.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestManifest {
    schema_version: u32,
    image_version: String,
    architecture: Architecture,
    rootfs_format: RootFsFormat,
    components: GuestComponentVersions,
    kernel_sha256: String,
    rootfs_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Architecture {
    Aarch64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RootFsFormat {
    Erofs,
}

/// Versions of the guest base, the upstream engine bundle and the image tool.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestComponentVersions {
    /// Linux kernel release.
    pub linux: String,
    /// Alpine base release.
    pub alpine: String,
    /// Docker static bundle (also covers dockerd and docker-proxy).
    pub docker: String,
    /// BuildKit embedded in dockerd.
    pub buildkit: String,
    /// Upstream containerd and its shim.
    pub containerd: String,
    /// Upstream OCI runtime.
    pub runc: String,
    /// Upstream Tini shipped as docker-init.
    pub docker_init: String,
    /// Upstream image formatter release.
    pub erofs_utils: String,
}

/// Failure to parse, generate or check a guest manifest.
#[derive(Debug, Error)]
pub enum GuestManifestError {
    /// The input exceeds the size limit.
    #[error("guest manifest exceeds the size limit")]
    TooLarge,
    /// JSON is invalid or does not match the manifest schema.
    #[error("invalid guest manifest JSON")]
    Json(#[from] serde_json::Error),
    /// This reader does not support the declared manifest schema.
    #[error("unsupported guest manifest schema {0}")]
    UnsupportedSchema(u32),
    /// The image or component version is invalid.
    #[error("invalid guest version field {0}")]
    InvalidVersion(&'static str),
    /// An artifact digest is not a lowercase SHA-256 digest.
    #[error("invalid guest SHA-256 digest for {0}")]
    InvalidChecksum(&'static str),
    /// Reading the manifest or an artifact failed.
    #[error("could not read {artifact}")]
    Read {
        /// Logical artifact name.
        artifact: &'static str,
        /// The underlying read error.
        #[source]
        source: io::Error,
    },
    /// The SHA-256 digest of an artifact does not match its manifest entry.
    #[error("guest checksum mismatch for {artifact}")]
    ChecksumMismatch {
        /// Logical artifact name.
        artifact: &'static str,
    },
}

impl GuestManifest {
    /// Creates metadata by hashing the exact bytes that will be packaged.
    pub fn create(
        image_version: String,
        components: GuestComponentVersions,
        kernel: impl Read,
        rootfs: impl Read,
    ) -> Result<Self, GuestManifestError> {
        let manifest = Self {
            schema_version: SCHEMA_VERSION,
            image_version,
            architecture: Architecture::Aarch64,
            rootfs_format: RootFsFormat::Erofs,
            components,
            kernel_sha256: sha256(KERNEL, kernel)?,
            rootfs_sha256: sha256(ROOTFS, rootfs)?,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parses metadata and validates its size, schema, versions and digest syntax.
    pub fn parse(bytes: &[u8]) -> Result<Self, GuestManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(GuestManifestError::TooLarge);
        }
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Reads and parses a manifest with a bounded input buffer.
    pub fn read(reader: impl Read) -> Result<Self, GuestManifestError> {
        let mut bytes = Vec::new();
        reader
            .take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|source| GuestManifestError::Read {
                artifact: "guest-manifest.json",
                source,
            })?;
        Self::parse(&bytes)
    }

    /// Checks the kernel and root filesystem bytes against their manifest digests.
    pub fn verify(&self, kernel: impl Read, rootfs: impl Read) -> Result<(), GuestManifestError> {
        for (artifact, actual, expected) in [
            (KERNEL, sha256(KERNEL, kernel)?, &self.kernel_sha256),
            (ROOTFS, sha256(ROOTFS, rootfs)?, &self.rootfs_sha256),
        ] {
            if actual != *expected {
                return Err(GuestManifestError::ChecksumMismatch { artifact });
            }
        }
        Ok(())
    }

    /// Encodes the schema as human-readable JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, GuestManifestError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Returns the app and guest image release version.
    pub fn image_version(&self) -> &str {
        &self.image_version
    }

    /// Returns the component versions recorded in this image.
    pub fn components(&self) -> &GuestComponentVersions {
        &self.components
    }

    fn validate(&self) -> Result<(), GuestManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(GuestManifestError::UnsupportedSchema(self.schema_version));
        }
        let versions = &self.components;
        for (name, value) in [
            ("image_version", &self.image_version),
            ("linux", &versions.linux),
            ("alpine", &versions.alpine),
            ("docker", &versions.docker),
            ("buildkit", &versions.buildkit),
            ("containerd", &versions.containerd),
            ("runc", &versions.runc),
            ("docker_init", &versions.docker_init),
            ("erofs_utils", &versions.erofs_utils),
        ] {
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-+_".contains(&byte))
            {
                return Err(GuestManifestError::InvalidVersion(name));
            }
        }
        for (artifact, value) in [(KERNEL, &self.kernel_sha256), (ROOTFS, &self.rootfs_sha256)] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(GuestManifestError::InvalidChecksum(artifact));
            }
        }
        Ok(())
    }
}

fn sha256(artifact: &'static str, mut source: impl Read) -> Result<String, GuestManifestError> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let size = source
            .read(&mut buffer)
            .map_err(|source| GuestManifestError::Read { artifact, source })?;
        if size == 0 {
            let mut hex = String::with_capacity(64);
            const DIGITS: &[u8; 16] = b"0123456789abcdef";
            for byte in hash.finalize() {
                hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
                hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
            }
            return Ok(hex);
        }
        hash.update(&buffer[..size]);
    }
}
