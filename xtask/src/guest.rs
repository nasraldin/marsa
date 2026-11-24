//! Drive the guest-input builder inside a pinned Linux container.

use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use marsa_core::guest_manifest::{GuestComponentVersions, GuestManifest};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Versions {
    kernel: Release,
    alpine: Alpine,
    docker: Docker,
    moby: Checksum,
    erofs: Version,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    version: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Docker {
    version: String,
    sha256: String,
    buildkit: String,
    containerd: String,
    runc: String,
    docker_init: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: String,
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
            println!("Usage: cargo xtask guest <kernel|rootfs|image|provenance|all>");
            return ExitCode::SUCCESS;
        }
        [arg] => match arg.to_str() {
            Some(stage @ ("kernel" | "rootfs" | "image" | "provenance" | "all")) => stage,
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
        "cargo xtask guest expects kernel, rootfs, image, provenance, or all. Run cargo xtask guest --help."
    );
    ExitCode::from(2)
}

fn build(stage: &str) -> io::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let guest = workspace.join("guest");
    let pins = fs::read_to_string(guest.join("versions.toml"))?;
    let versions: Versions = toml::from_str(&pins).map_err(io::Error::other)?;
    let output = workspace.join("target/guest");
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    if stage == "provenance" {
        return write_provenance(&guest, &output);
    }
    let mount = format!("type=bind,source={},target=/output", output.display());
    let user = format!("{}:{}", native_id("-u")?, native_id("-g")?);
    if stage == "image" {
        return assemble_image(&guest, &output, &user, &versions);
    }
    let inputs = versions.inputs()?;
    let image = docker_image(&guest, &["--target", "inputs"])?;
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--pull=never",
            "--read-only",
            "--cap-drop=ALL",
            "--tmpfs",
            "/tmp:rw,exec,nosuid,nodev,mode=1777",
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
        let image = docker_image(&guest.join("kernel"), &[])?;
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
    if stage == "all" {
        assemble_image(&guest, &output, &user, &versions)?;
        write_provenance(&guest, &output)?;
    }
    Ok(())
}

fn write_provenance(guest: &Path, output: &Path) -> io::Result<()> {
    let staging = tempfile::tempdir_in(output)?;
    let status = Command::new("python3")
        .arg(guest.join("provenance/build.py"))
        .arg(output)
        .arg(staging.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .status()
        .map_err(|error| {
            io::Error::new(error.kind(), format!("could not run Python 3: {error}"))
        })?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "guest provenance generation exited with {status}"
        )));
    }
    for artifact in [
        "guest-sbom.cdx.json",
        "guest-sources.json",
        "guest-notices.tar",
    ] {
        fs::rename(staging.path().join(artifact), output.join(artifact))?;
    }
    println!("Wrote guest SBOM, source references and licence archive");
    Ok(())
}

fn assemble_image(guest: &Path, output: &Path, user: &str, versions: &Versions) -> io::Result<()> {
    let kernel = File::open(output.join("Image")).map_err(|error| {
        io::Error::other(format!(
            "could not read Image (run cargo xtask guest kernel): {error}"
        ))
    })?;
    let rootfs_tar = output.join("rootfs.tar");
    if !rootfs_tar.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "missing rootfs.tar (run cargo xtask guest rootfs)",
        ));
    }
    let staging = tempfile::tempdir_in(output)?;
    let stage_mount = format!(
        "type=bind,source={},target=/output",
        staging.path().display()
    );
    let tar_mount = format!(
        "type=bind,source={},target=/rootfs.tar,readonly",
        rootfs_tar.display()
    );
    let workspace = guest.join("..").canonicalize()?;
    let source_commit = exact_source_commit(&workspace);
    let bin_dir = build_guest_programs(&workspace, output)?;
    let init_mount = format!(
        "type=bind,source={},target=/programs/marsa-init,readonly",
        bin_dir.join("marsa-init").display()
    );
    let agent_mount = format!(
        "type=bind,source={},target=/programs/marsa-agent,readonly",
        bin_dir.join("marsa-agent").display()
    );
    let image = docker_image(guest, &["--target", "image"])?;
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--pull=never",
            "--read-only",
            "--network=none",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,noexec,mode=1777",
        ])
        .args([
            "--user",
            user,
            "--mount",
            &stage_mount,
            "--mount",
            &tar_mount,
        ])
        .args(["--mount", &init_mount, "--mount", &agent_mount])
        .args([&image, &versions.erofs.version])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "image builder exited with {status}"
        )));
    }

    let rootfs_path = staging.path().join("rootfs.img");
    let rootfs = File::open(&rootfs_path)?;
    let manifest = GuestManifest::create(
        env!("CARGO_PKG_VERSION").to_owned(),
        versions.components(),
        source_commit,
        kernel,
        rootfs,
    )
    .map_err(io::Error::other)?;
    let manifest_path = staging.path().join("guest-manifest.json");
    {
        let mut file = File::create(&manifest_path)?;
        file.write_all(&manifest.to_json().map_err(io::Error::other)?)?;
        file.write_all(b"\n")?;
    }
    fs::rename(
        staging.path().join("image-rootfs.tar"),
        output.join("image-rootfs.tar"),
    )?;
    fs::rename(rootfs_path, output.join("rootfs.img"))?;
    fs::rename(manifest_path, output.join("guest-manifest.json"))?;
    let input_digest = "rootfs-input.sha256";
    fs::rename(staging.path().join(input_digest), output.join(input_digest))?;
    println!("Wrote rootfs.img and guest-manifest.json");
    Ok(())
}

fn exact_source_commit(workspace: &Path) -> Option<String> {
    let mut git = Command::new("git");
    git.current_dir(workspace);
    let status = git
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .ok()?;
    if !status.status.success() || !status.stdout.is_empty() {
        return None;
    }
    let revision = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !revision.status.success() {
        return None;
    }
    String::from_utf8(revision.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn build_guest_programs(workspace: &Path, output: &Path) -> io::Result<PathBuf> {
    const TARGET: &str = "aarch64-unknown-linux-musl";
    let target_dir = output
        .parent()
        .ok_or_else(|| io::Error::other("missing target directory"))?;
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::home_dir().map(|home| home.join(".cargo")))
        .ok_or_else(|| io::Error::other("could not find Cargo home"))?
        .canonicalize()?;
    let remap_flags = format!(
        "--remap-path-prefix={}=/marsa-source\u{1f}--remap-path-prefix={}=/cargo-source",
        workspace.display(),
        cargo_home.display(),
    );
    let mut build = Command::new("cargo");
    build
        .current_dir(workspace)
        .env("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER", "rust-lld")
        .env("CARGO_ENCODED_RUSTFLAGS", remap_flags)
        .env("CC_aarch64_unknown_linux_musl", "clang")
        .env("AR_aarch64_unknown_linux_musl", guest_archiver(workspace)?)
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            TARGET,
            "--target-dir",
        ])
        .arg(target_dir)
        .args(["-p", "marsa-init", "-p", "marsa-agent"]);
    if cfg!(target_os = "macos") {
        // Apple clang's musl stddef wrapper includes a system header that
        // ring's freestanding build deliberately excludes. Select the
        // compiler's freestanding definitions without changing the target ABI.
        build.env("CFLAGS_aarch64_unknown_linux_musl", "-U__musl__");
    }
    let status = build.status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "guest Rust build exited with {status}"
        )));
    }
    Ok(target_dir.join(TARGET).join("release"))
}

fn guest_archiver(workspace: &Path) -> io::Result<PathBuf> {
    let output = Command::new("rustc")
        .current_dir(workspace)
        .args(["--print", "target-libdir"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("rustc could not locate guest build tools"));
    }
    let directory = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let archiver = Path::new(directory.trim())
        .parent()
        .ok_or_else(|| io::Error::other("rustc returned an invalid host library directory"))?
        .join("bin")
        .join("llvm-ar");
    if !archiver.is_file() {
        return Err(io::Error::other(
            "install the pinned Rust llvm-tools component to build the guest",
        ));
    }
    Ok(archiver)
}

impl Versions {
    fn components(&self) -> GuestComponentVersions {
        GuestComponentVersions {
            linux: self.kernel.version.clone(),
            alpine: self.alpine.version.clone(),
            docker: self.docker.version.clone(),
            buildkit: self.docker.buildkit.clone(),
            containerd: self.docker.containerd.clone(),
            runc: self.docker.runc.clone(),
            docker_init: self.docker.docker_init.clone(),
            erofs_utils: self.erofs.version.clone(),
            marsa_init: Some(env!("CARGO_PKG_VERSION").to_owned()),
            marsa_agent: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }
    }

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

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
pub(super) fn verify_release_manifest(pins: &str, manifest: &GuestManifest) -> io::Result<()> {
    let versions: Versions = toml::from_str(pins).map_err(io::Error::other)?;
    if manifest.source_commit().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "release guest manifest has no source commit",
        ));
    }
    if manifest.image_version() != env!("CARGO_PKG_VERSION")
        || manifest.components() != &versions.components()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "release guest does not match the pinned component and workspace versions",
        ));
    }
    Ok(())
}

fn docker_image(guest: &Path, args: &[&str]) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["build", "--quiet"])
        .args(args)
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

pub(super) fn digest(value: &str) -> io::Result<()> {
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
