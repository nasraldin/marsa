//! Verify official Docker host artifacts before staging them for app packaging.

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Versions {
    docker: Pin,
    compose: Pin,
    buildx: Pin,
    credential_helper: Pin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    version: String,
    sha256: String,
}

struct Artifact<'a> {
    pin: &'a Pin,
    repository: &'static str,
    license: &'static str,
    path: &'static str,
    url: String,
    archive_member: Option<&'static str>,
}

const SBOM: &str = "upstream-sbom-input.json";

pub(super) fn run(args: &[OsString]) -> ExitCode {
    match args {
        [flag] if flag == "--help" || flag == "-h" => {
            println!(
                "Usage: cargo xtask fetch-upstream\n\nStages verified macOS ARM64 binaries and SBOM input under target/upstream/."
            );
            return ExitCode::SUCCESS;
        }
        [] => {}
        _ => {
            eprintln!("Run cargo xtask fetch-upstream --help for usage.");
            return ExitCode::from(2);
        }
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let result = (|| {
        let pins = fs::read_to_string(workspace.join("packaging/upstream-versions.toml"))?;
        fetch(&pins, &workspace.join("target/upstream"), download)
    })();
    match result {
        Ok(output) => {
            println!("Staged verified upstream binaries in {}", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cargo xtask fetch-upstream: {error}");
            ExitCode::FAILURE
        }
    }
}

fn fetch(
    pins: &str,
    output: &Path,
    download: impl Fn(&str, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let versions = Versions::parse(pins)?;
    let artifacts = versions.artifacts();

    fs::create_dir_all(output)?;
    let output = output.canonicalize()?;
    let target = output.join(format!("darwin-arm64-{}", sha256(pins.as_bytes())?));
    let temporary = tempfile::tempdir_in(&output)?;
    let bundle = temporary.path().join("bundle");
    let sbom = stage_verified(&artifacts, temporary.path(), &bundle, download)?;
    fs::write(bundle.join(SBOM), &sbom)?;

    // Rename publishes the verified bundle as a single directory. A concurrent
    // fetch with the same pins may have already published an identical result.
    if let Err(rename_error) = fs::rename(&bundle, &target) {
        if !target.is_dir() {
            return Err(rename_error);
        }
        for artifact in &artifacts {
            if file_hash(&bundle.join(artifact.path))? != file_hash(&target.join(artifact.path))? {
                return Err(io::Error::other(format!(
                    "existing staged binary {} does not match the verified download",
                    target.join(artifact.path).display()
                )));
            }
        }
        if fs::read(target.join(SBOM))? != sbom {
            return Err(io::Error::other(
                "existing upstream SBOM input does not match",
            ));
        }
    }
    Ok(target)
}

/// Stage fresh supplier downloads under the caller's private release directory.
#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
pub(super) fn stage_release(
    pins: &str,
    work: &Path,
    resources: &Path,
    output: &Path,
) -> io::Result<()> {
    let versions = Versions::parse(pins)?;
    let sbom = stage_verified(&versions.artifacts(), work, resources, download)?;
    fs::write(output.join(SBOM), sbom)
}

fn stage_verified(
    artifacts: &[Artifact<'_>],
    work: &Path,
    resources: &Path,
    download: impl Fn(&str, &Path) -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    fs::create_dir_all(resources.join("bin/cli-plugins"))?;
    let payload = work.join("download");
    let mut components = Vec::new();
    for artifact in artifacts {
        let hash = artifact.stage(&download, &payload, resources)?;
        components.push(artifact.component(&hash));
    }
    let mut sbom = serde_json::to_vec_pretty(&json!({ "components": components }))
        .map_err(io::Error::other)?;
    sbom.push(b'\n');
    Ok(sbom)
}

impl Versions {
    fn parse(pins: &str) -> io::Result<Self> {
        let versions: Self = toml::from_str(pins).map_err(io::Error::other)?;
        for artifact in versions.artifacts() {
            artifact.pin.validate(artifact.path)?;
        }
        Ok(versions)
    }

    fn artifacts(&self) -> [Artifact<'_>; 4] {
        let docker = &self.docker.version;
        let compose = &self.compose.version;
        let buildx = &self.buildx.version;
        let helper = &self.credential_helper.version;
        [
            Artifact {
                pin: &self.docker,
                repository: "cli",
                license: "Apache-2.0",
                path: "bin/docker",
                url: format!(
                    "https://download.docker.com/mac/static/stable/aarch64/docker-cli_{docker}.tgz"
                ),
                archive_member: Some("bin/docker"),
            },
            Artifact {
                pin: &self.compose,
                repository: "compose",
                license: "Apache-2.0",
                path: "bin/cli-plugins/docker-compose",
                url: format!(
                    "https://github.com/docker/compose/releases/download/v{compose}/docker-compose-darwin-aarch64"
                ),
                archive_member: None,
            },
            Artifact {
                pin: &self.buildx,
                repository: "buildx",
                license: "Apache-2.0",
                path: "bin/cli-plugins/docker-buildx",
                url: format!(
                    "https://github.com/docker/buildx/releases/download/v{buildx}/buildx-v{buildx}.darwin-arm64"
                ),
                archive_member: None,
            },
            Artifact {
                pin: &self.credential_helper,
                repository: "docker-credential-helpers",
                license: "MIT",
                path: "bin/docker-credential-osxkeychain",
                url: format!(
                    "https://github.com/docker/docker-credential-helpers/releases/download/v{helper}/docker-credential-osxkeychain-v{helper}.darwin-arm64"
                ),
                archive_member: None,
            },
        ]
    }
}

impl Pin {
    fn validate(&self, path: &str) -> io::Result<()> {
        let parts = self.version.split('.').collect::<Vec<_>>();
        if parts.len() != 3
            || !parts.iter().all(|part| {
                part.parse::<u32>()
                    .is_ok_and(|value| value.to_string() == *part)
            })
        {
            return Err(io::Error::other(format!(
                "{path}: expected a stable release version"
            )));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(io::Error::other(format!(
                "{path}: expected a lowercase SHA-256 digest"
            )));
        }
        Ok(())
    }
}

impl Artifact<'_> {
    fn stage(
        &self,
        download: impl Fn(&str, &Path) -> io::Result<()>,
        payload: &Path,
        bundle: &Path,
    ) -> io::Result<String> {
        download(&self.url, payload)?;
        let actual = file_hash(payload)?;
        if actual != self.pin.sha256 {
            return Err(io::Error::other(format!(
                "{} SHA-256 mismatch: expected {}, got {actual}",
                self.path, self.pin.sha256
            )));
        }
        let executable = bundle.join(self.path);
        let hash = if let Some(member) = self.archive_member {
            let status = Command::new("tar")
                .arg("-xOf")
                .arg(payload)
                .arg(member)
                .stdout(Stdio::from(File::create(&executable)?))
                .status()?;
            if !status.success() {
                return Err(io::Error::other(format!(
                    "could not extract {member}: {status}"
                )));
            }
            fs::remove_file(payload)?;
            file_hash(&executable)?
        } else {
            fs::rename(payload, &executable)?;
            actual
        };
        #[cfg(unix)]
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
        Ok(hash)
    }

    fn component(&self, hash: &str) -> Value {
        let repo = self.repository;
        let version = &self.pin.version;
        json!({
            "type": "application",
            "name": Path::new(self.path).file_name().and_then(|name| name.to_str()),
            "version": version,
            "purl": format!("pkg:github/docker/{repo}@v{version}"),
            "licenses": [{ "license": { "id": self.license } }],
            "hashes": [{ "alg": "SHA-256", "content": hash }],
            "externalReferences": [
                {
                    "type": "distribution",
                    "url": self.url,
                    "hashes": [{ "alg": "SHA-256", "content": self.pin.sha256 }]
                },
                {
                    "type": "license",
                    "url": format!("https://github.com/docker/{repo}/blob/v{version}/LICENSE")
                }
            ]
        })
    }
}

fn download(url: &str, destination: &Path) -> io::Result<()> {
    let status = Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "300",
            "--max-filesize",
            "134217728",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "download failed for {url}: {status}"
        )));
    }
    Ok(())
}

fn file_hash(path: &Path) -> io::Result<String> {
    sha256(File::open(path)?)
}

pub(super) fn sha256(mut source: impl Read) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let size = source.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    let mut hex = String::with_capacity(64);
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in hash.finalize() {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_mismatch_publishes_no_bundle() {
        let output = tempfile::tempdir().expect("create output directory");
        let pins = include_str!("../../packaging/upstream-versions.toml");
        let error = fetch(pins, output.path(), |_, destination| {
            fs::write(destination, b"corrupt download")
        })
        .expect_err("reject an artifact with the wrong hash");
        assert!(
            error
                .to_string()
                .starts_with("bin/docker SHA-256 mismatch:")
        );
        assert!(
            fs::read_dir(output.path())
                .expect("list output directory")
                .next()
                .is_none(),
            "the failed fetch left staged or published files"
        );
    }
}
