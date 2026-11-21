//! Drive the guest-input builder inside a pinned Linux container.

use std::{
    ffi::OsString,
    fs, io,
    path::Path,
    process::{Command, ExitCode},
};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Versions {
    kernel: Release,
    alpine: Alpine,
    docker: Release,
    moby: Checksum,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    version: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checksum {
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Alpine {
    version: String,
    sha256: String,
    packages: Vec<String>,
}

pub fn run(args: &[OsString]) -> ExitCode {
    let stage = match args {
        [arg] if arg == "--help" || arg == "-h" => {
            println!("Usage: cargo xtask guest <kernel|rootfs|image|all>");
            return ExitCode::SUCCESS;
        }
        [arg] => match arg.to_str() {
            Some(stage @ ("kernel" | "rootfs" | "image" | "all")) => stage,
            _ => return usage_error(),
        },
        _ => return usage_error(),
    };
    if let Err(error) = build(stage) {
        eprintln!("cargo xtask guest: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn usage_error() -> ExitCode {
    eprintln!(
        "cargo xtask guest expects kernel, rootfs, image, or all. Run cargo xtask guest --help."
    );
    ExitCode::from(2)
}

fn build(stage: &str) -> io::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let guest = workspace.join("guest");
    let pins = fs::read_to_string(guest.join("versions.toml"))?;
    let versions: Versions = toml::from_str(&pins).map_err(io::Error::other)?;
    let inputs = versions.inputs()?;
    let output = workspace.join("target/guest");
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let mount = format!("type=bind,source={},target=/output", output.display());
    let user = format!("{}:{}", native_id("-u")?, native_id("-g")?);
    let image = docker_image(&guest)?;
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--pull=never",
            "--read-only",
            "--cap-drop=ALL",
        ])
        .args(["--security-opt=no-new-privileges", "--user", &user])
        .args(["--mount", &mount, &image, stage])
        .args(inputs)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "guest builder exited with {status}"
        )));
    }
    if matches!(stage, "kernel" | "all") {
        let image = docker_image(&guest.join("kernel"))?;
        let status = Command::new("docker")
            .args([
                "run",
                "--rm",
                "--pull=never",
                "--network=none",
                "--cap-drop=ALL",
            ])
            .args(["--security-opt=no-new-privileges", "--user", &user])
            .args([
                "--mount",
                &mount,
                &image,
                &versions.kernel.version,
                &versions.kernel.sha256,
                &versions.moby.sha256,
            ])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "kernel builder exited with {status}"
            )));
        }
    }
    Ok(())
}

impl Versions {
    fn inputs(&self) -> io::Result<[String; 8]> {
        let kernel = version(&self.kernel.version)?;
        let alpine = version(&self.alpine.version)?;
        version(&self.docker.version)?;
        for checksum in [
            &self.kernel.sha256,
            &self.alpine.sha256,
            &self.docker.sha256,
            &self.moby.sha256,
        ] {
            digest(checksum)?;
        }
        if self.alpine.packages.is_empty()
            || self.alpine.packages.iter().any(|name| {
                name.is_empty()
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_+.".contains(&byte))
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Alpine package list",
            ));
        }
        let kernel_version = &self.kernel.version;
        let alpine_version = &self.alpine.version;
        let docker_version = &self.docker.version;
        Ok([
            self.kernel.sha256.clone(),
            format!(
                "https://cdn.kernel.org/pub/linux/kernel/v{}.x/linux-{kernel_version}.tar.xz",
                kernel[0]
            ),
            self.alpine.sha256.clone(),
            format!(
                "https://dl-cdn.alpinelinux.org/alpine/v{}.{}/releases/aarch64/alpine-minirootfs-{alpine_version}-aarch64.tar.gz",
                alpine[0], alpine[1]
            ),
            self.docker.sha256.clone(),
            format!(
                "https://download.docker.com/linux/static/stable/aarch64/docker-{docker_version}.tgz"
            ),
            self.moby.sha256.clone(),
            format!(
                "https://raw.githubusercontent.com/moby/moby/docker-v{docker_version}/contrib/check-config.sh"
            ),
        ])
    }
}

fn docker_image(guest: &Path) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["build", "--quiet"])
        .arg(guest)
        .output()
        .map_err(|error| io::Error::new(error.kind(), format!("could not run Docker: {error}")))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "Docker build failed: {}",
            reason.trim()
        )));
    }
    let response = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let image = response.trim();
    digest(image.strip_prefix("sha256:").unwrap_or_default())?;
    Ok(image.to_owned())
}

fn native_id(flag: &str) -> io::Result<u32> {
    let output = Command::new("id").arg(flag).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("id {flag} failed")));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(io::Error::other)
}

fn digest(value: &str) -> io::Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "expected a lowercase SHA-256 digest",
    ))
}

fn version(value: &str) -> io::Result<[u32; 3]> {
    let parts = value
        .split('.')
        .map(|part| {
            let number: u32 = part.parse().ok()?;
            (number.to_string() == part).then_some(number)
        })
        .collect::<Option<Vec<_>>>();
    parts
        .and_then(|parts| parts.try_into().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a three-part release version",
            )
        })
}
