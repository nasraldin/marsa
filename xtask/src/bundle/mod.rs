//! Assemble the unsigned native release from fixed, verified input roles.

mod resources;

use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::ExitCode,
};

use crate::{guest, release_resources::platform};

const USAGE: &str = "Usage: cargo xtask bundle --host-dir DIR --network-helper FILE --resources-dir STAGE --guest-manifest-sha256 HEX [--licenses-dir DIR] --output NEW_Marsa.app";
const HOST_ROLES: [&str; 4] = ["marsa-gui", "marsad", "marsa", "marsa-mcp"];
const LAUNCH_AGENT: &str = "io.github.nasraldin.marsa.marsad.plist";
const NATIVE_LIMIT: u64 = 512 * 1024 * 1024;
const TEXT_LIMIT: u64 = 4 * 1024 * 1024;

struct Inputs<'a> {
    host_dir: &'a Path,
    helper: &'a Path,
    stage: &'a Path,
    guest_sha256: &'a str,
    output: &'a Path,
    licenses: Option<&'a Path>,
}

pub(super) fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") {
        println!(
            "{USAGE}\n\nAssemble an unsigned macOS ARM64 app using the verified M10-12 resource stage."
        );
        return ExitCode::SUCCESS;
    }
    let Some(inputs) = parse(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent();
    let result = workspace
        .ok_or_else(|| invalid("could not locate the workspace"))
        .and_then(|workspace| assemble(&inputs, workspace));
    match result {
        Ok(bytes) => {
            println!("Bundle: {}", inputs.output.display());
            println!("Bundle size: {bytes} bytes (logical file sizes)");
            if inputs.licenses.is_none() {
                println!("Combined release notices and SBOM were not supplied.");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cargo xtask bundle: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[OsString]) -> Option<Inputs<'_>> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let (mut host, mut helper, mut stage, mut hash, mut output, mut licenses) =
        (None, None, None, None, None, None);
    for [flag, value] in pairs {
        if value.is_empty() {
            return None;
        }
        let path = Path::new(value);
        match flag.to_str()? {
            "--host-dir" if host.is_none() => host = Some(path),
            "--network-helper" if helper.is_none() => helper = Some(path),
            "--resources-dir" if stage.is_none() => stage = Some(path),
            "--guest-manifest-sha256" if hash.is_none() => hash = Some(value.to_str()?),
            "--output" if output.is_none() => output = Some(path),
            "--licenses-dir" if licenses.is_none() => licenses = Some(path),
            _ => return None,
        }
    }
    Some(Inputs {
        host_dir: host?,
        helper: helper?,
        stage: stage?,
        guest_sha256: hash?,
        output: output?,
        licenses,
    })
}

fn assemble(inputs: &Inputs<'_>, workspace: &Path) -> io::Result<u64> {
    guest::digest(inputs.guest_sha256)?;
    let output = std::path::absolute(inputs.output)?;
    if output.extension() != Some(OsStr::new("app")) {
        return Err(invalid("bundle output must have an .app extension"));
    }
    let name = output
        .file_name()
        .ok_or_else(|| invalid("expected a bundle name"))?;
    let parent_path = output
        .parent()
        .ok_or_else(|| invalid("expected an output parent"))?;
    let parent = platform::directory(parent_path)?;
    platform::owned_output_parent(&parent)?;
    platform::require_absent(&parent, name)?;

    let host = platform::directory(inputs.host_dir)?;
    let helper = open_named_path(inputs.helper)?;
    let stage = resources::Staged::open(inputs.stage)?;
    let licenses = inputs.licenses.map(std::path::absolute).transpose()?;

    platform::with_current_directory(&parent, || {
        let scratch = platform::Scratch::create()?;
        let output = scratch.path().join("publish");
        let contents = output.join("Contents");
        let binaries = contents.join("MacOS");
        let helpers = contents.join("Helpers");
        let resources_dir = contents.join("Resources");
        let agents = contents.join("Library/LaunchAgents");
        for directory in [
            &binaries,
            &helpers,
            &agents,
            &resources_dir.join("guest"),
            &resources_dir.join("bin/cli-plugins"),
            &resources_dir.join("licenses"),
            &resources_dir.join("fonts"),
            &resources_dir.join("locales/en-US"),
            &resources_dir.join("locales/ar"),
        ] {
            fs::create_dir_all(directory)?;
        }

        for role in HOST_ROLES {
            copy_executable(platform::input_file(&host, role)?, &binaries.join(role))?;
        }
        copy_executable(helper, &helpers.join("marsa-net-helper"))?;
        stage.install(&resources_dir, inputs.guest_sha256)?;
        resources::install_workspace_assets(workspace, &resources_dir, licenses.as_deref())?;

        let agent_source = platform::directory(&workspace.join("packaging/macos/LaunchAgents"))?;
        copy_file(
            platform::input_file(&agent_source, LAUNCH_AGENT)?,
            &agents.join(LAUNCH_AGENT),
            TEXT_LIMIT,
            0o644,
        )?;
        let plist = include_str!("../../../packaging/macos/Info.plist")
            .replace("@VERSION@", env!("CARGO_PKG_VERSION"));
        write_file(&contents.join("Info.plist"), plist.as_bytes(), 0o644)?;

        let bytes = sync_tree(&output)?;
        let prepared = platform::directory(scratch.path())?;
        platform::publish_release(&prepared, &parent, parent_path, name)?;
        prepared
            .sync_all()
            .and_then(|()| parent.sync_all())
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("bundle was assembled, but directory sync failed: {error}"),
                )
            })?;
        Ok(bytes)
    })
}

fn open_named_path(path: &Path) -> io::Result<File> {
    let path = std::path::absolute(path)?;
    let directory = path
        .parent()
        .ok_or_else(|| invalid("expected a file parent"))?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid("expected a UTF-8 file name"))?;
    platform::input_file(&platform::directory(directory)?, name)
}

fn copy_executable(input: File, destination: &Path) -> io::Result<()> {
    if input.metadata()?.mode() & 0o111 == 0 {
        return Err(invalid(format!(
            "{}: input is not executable",
            destination.display()
        )));
    }
    copy_file(input, destination, NATIVE_LIMIT, 0o755)?;
    verify_native(destination)
}

fn verify_native(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0; 32];
    file.read_exact(&mut header)?;
    let word = |offset: usize| {
        u32::from_le_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ])
    };
    // Constants from Apple's mach/machine.h and mach-o/loader.h, also used
    // by the release signature verifier. Release roles are thin ARM64 executables.
    let count = u64::from(word(16));
    let command_bytes = u64::from(word(20));
    if word(0) != 0xfeed_facf
        || word(4) != 0x0100_000c
        || word(12) != 2
        || count == 0
        || command_bytes < 8 * count
        || command_bytes > file.metadata()?.len().saturating_sub(32)
    {
        return Err(invalid(format!(
            "{}: expected a native ARM64 executable",
            path.display()
        )));
    }
    Ok(())
}

fn copy_file(mut input: File, destination: &Path, limit: u64, mode: u32) -> io::Result<()> {
    let initial = input.metadata()?;
    if !(1..=limit).contains(&initial.len()) || initial.mode() & 0o6022 != 0 {
        return Err(invalid(format!(
            "{}: invalid input size or mode",
            destination.display()
        )));
    }
    let mut output = create_file(destination, mode)?;
    let copied = io::copy(&mut Read::by_ref(&mut input).take(limit + 1), &mut output)?;
    let current = input.metadata()?;
    if copied != initial.len()
        || current.len() != initial.len()
        || current.mtime() != initial.mtime()
        || current.mtime_nsec() != initial.mtime_nsec()
        || current.ctime() != initial.ctime()
        || current.ctime_nsec() != initial.ctime_nsec()
    {
        return Err(invalid(format!(
            "{}: input changed during assembly",
            destination.display()
        )));
    }
    Ok(())
}

fn write_file(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    create_file(path, mode)?.write_all(data)
}

fn create_file(path: &Path, mode: u32) -> io::Result<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.set_permissions(Permissions::from_mode(mode))?;
    Ok(file)
}

fn read_small(input: File, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > limit {
        return Err(invalid("invalid resource size"));
    }
    Ok(bytes)
}

fn sync_tree(path: &Path) -> io::Result<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let size = if kind.is_dir() {
            sync_tree(&entry.path())?
        } else if kind.is_file() {
            let file = File::open(entry.path())?;
            file.sync_all()?;
            file.metadata()?.len()
        } else {
            return Err(invalid("unexpected bundle entry"));
        };
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("bundle size overflow"))?;
    }
    fs::set_permissions(path, Permissions::from_mode(0o755))?;
    File::open(path)?.sync_all()?;
    Ok(total)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
