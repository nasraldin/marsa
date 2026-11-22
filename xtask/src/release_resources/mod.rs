//! Prepare the verified guest and Docker inputs consumed by macOS app assembly.

mod platform;

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::ExitCode,
};

use marsa_core::guest_manifest::GuestManifest;

use crate::{guest, upstream};

const MANIFEST: &str = "guest-manifest.json";
const KERNEL: &str = "Image";
const ROOTFS: &str = "rootfs.img";
const GUEST_INPUTS: &[&str] = &[KERNEL, ROOTFS, MANIFEST];
const MANIFEST_LIMIT: u64 = 16 * 1024;
const KERNEL_LIMIT: u64 = 64 * 1024 * 1024;
const ROOTFS_LIMIT: u64 = 1024 * 1024 * 1024;
const USAGE: &str = "Usage: cargo xtask stage-release-resources --guest-dir DIR --guest-manifest-sha256 HEX --output NEW_DIR";

struct Inputs<'a> {
    guest_dir: &'a Path,
    manifest_sha256: &'a str,
    output: &'a Path,
}

pub(super) fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") {
        println!("{USAGE}\n\nThe guest manifest SHA-256 must come from the trusted guest build.");
        return ExitCode::SUCCESS;
    }
    let Some(inputs) = parse(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    match stage(inputs, &workspace) {
        Ok(output) => {
            println!("Staged verified release resources in {}", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cargo xtask stage-release-resources: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[OsString]) -> Option<Inputs<'_>> {
    let (options, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let (mut guest_dir, mut manifest_sha256, mut output) = (None, None, None);
    for [name, value] in options {
        if value.is_empty() {
            return None;
        }
        match name.to_str()? {
            "--guest-dir" if guest_dir.is_none() => guest_dir = Some(Path::new(value)),
            "--guest-manifest-sha256" if manifest_sha256.is_none() => {
                manifest_sha256 = Some(value.to_str()?);
            }
            "--output" if output.is_none() => output = Some(Path::new(value)),
            _ => return None,
        }
    }
    Some(Inputs {
        guest_dir: guest_dir?,
        manifest_sha256: manifest_sha256?,
        output: output?,
    })
}

fn stage(inputs: Inputs<'_>, workspace: &Path) -> io::Result<PathBuf> {
    guest::digest(inputs.manifest_sha256)?;
    let source = platform::directory(inputs.guest_dir)?;
    platform::require_names(&source, GUEST_INPUTS)?;
    let output_name = inputs
        .output
        .file_name()
        .ok_or(io::ErrorKind::InvalidInput)?;
    let parent_path = inputs.output.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let parent = platform::directory(parent_path)?;
    platform::owned_output_parent(&parent)?;
    platform::require_absent(&parent, output_name)?;
    let source_metadata = source.metadata()?;
    let parent_metadata = parent.metadata()?;
    if source_metadata.dev() == parent_metadata.dev()
        && source_metadata.ino() == parent_metadata.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "release output cannot be inside the guest handoff",
        ));
    }

    let work = tempfile::Builder::new()
        .prefix(".marsa-release-")
        .tempdir_in(parent_path)?;
    let output = work.path().join("publish");
    let resources = output.join("Resources");
    let staged_guest = resources.join("guest");
    fs::create_dir_all(&staged_guest)?;
    fs::set_permissions(&output, Permissions::from_mode(0o700))?;
    let pins = fs::read_to_string(workspace.join("guest/versions.toml"))?;
    stage_guest(&source, inputs.manifest_sha256, &pins, &staged_guest)?;
    let pins = fs::read_to_string(workspace.join("packaging/upstream-versions.toml"))?;
    upstream::stage_release(&pins, work.path(), &resources, &output)?;

    sync_tree(&output)?;
    let temporary = platform::directory(work.path())?;
    platform::publish(&temporary, "publish", &parent, output_name)?;
    parent.sync_all().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("release resources were published, but directory sync failed: {error}"),
        )
    })?;
    Ok(inputs.output.to_owned())
}

fn stage_guest(source: &File, expected: &str, pins: &str, output: &Path) -> io::Result<()> {
    let mut manifest_bytes = Vec::new();
    platform::input_file(source, MANIFEST)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut manifest_bytes)?;
    if manifest_bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(invalid("release guest manifest exceeds the size limit"));
    }
    if upstream::sha256(manifest_bytes.as_slice())? != expected {
        return Err(invalid("trusted guest manifest SHA-256 mismatch"));
    }
    let manifest = GuestManifest::parse(&manifest_bytes).map_err(io::Error::other)?;
    guest::verify_release_manifest(pins, &manifest)?;

    let kernel = copy_input(source, output, KERNEL, 64, KERNEL_LIMIT)?;
    let rootfs = copy_input(source, output, ROOTFS, 4096, ROOTFS_LIMIT)?;
    manifest
        .verify(&kernel, &rootfs)
        .map_err(io::Error::other)?;
    let mut kernel_magic = [0u8; 4];
    let mut rootfs_magic = [0u8; 4];
    kernel.read_exact_at(&mut kernel_magic, 56)?;
    rootfs.read_exact_at(&mut rootfs_magic, 1024)?;
    if kernel_magic != *b"ARM\x64"
        || rootfs_magic != 0xe0f5_e1e2u32.to_le_bytes()
        || !rootfs.metadata()?.len().is_multiple_of(512)
    {
        return Err(invalid(
            "release guest must contain an ARM64 Image and an EROFS root disk",
        ));
    }
    create_input(output, MANIFEST)?.write_all(&manifest_bytes)
}

fn copy_input(source: &File, output: &Path, name: &str, min: u64, max: u64) -> io::Result<File> {
    let input = platform::input_file(source, name)?;
    if !(min..=max).contains(&input.metadata()?.len()) {
        return Err(invalid(format!("invalid guest artifact size for {name}")));
    }
    let mut target = create_input(output, name)?;
    let count = io::copy(&mut input.take(max + 1), &mut target)?;
    if !(min..=max).contains(&count) {
        return Err(invalid(format!("invalid guest artifact size for {name}")));
    }
    File::open(output.join(name))
}

fn create_input(output: &Path, name: &str) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join(name))?;
    file.set_permissions(Permissions::from_mode(0o644))?;
    Ok(file)
}

fn sync_tree(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            sync_tree(&entry.path())?;
        } else if kind.is_file() {
            File::open(entry.path())?.sync_all()?;
        } else {
            return Err(invalid("unexpected staged resource type"));
        }
    }
    File::open(root)?.sync_all()
}

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
