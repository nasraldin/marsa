//! Resolve host files before asking the framework to consume the configuration.

use std::{
<<<<<<< HEAD
    fmt::Write as _,
=======
>>>>>>> origin/main
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

<<<<<<< HEAD
use marsa_core::guest_boot::Settings;
use marsa_vm::{NetworkSpec, ShareSpec, VmError, VmSpec};
=======
use marsa_core::guest_boot::{ARM64_CMDLINE_BYTES, Settings, append_share_arg};
use marsa_vm::{ShareSpec, VmError, VmSpec};
>>>>>>> origin/main

use super::state::invalid_spec;

pub(super) const MIB: u64 = 1024 * 1024;

pub(super) struct Prepared {
    pub(super) spec: VmSpec,
    pub(super) console_out: File,
    pub(super) discover_network: bool,
}

pub(super) fn prepare(mut spec: VmSpec) -> Result<Prepared, VmError> {
    spec.validate()?;
    if !spec.memory_bytes.is_multiple_of(MIB) {
        return Err(invalid_spec(
            "memory_bytes",
            "memory must be a multiple of 1 MiB",
        ));
    }
    let boot = prepare_shares(&mut spec)?;
<<<<<<< HEAD
    if spec.network != NetworkSpec::Nat {
        return Err(unsupported("VZ userspace networking"));
    }
=======
>>>>>>> origin/main

    let mut kernel = open_regular("kernel", &spec.kernel, false)?;
    let mut header = [0; 64];
    if let Err(error) = kernel.read_exact(&mut header) {
        return Err(invalid_spec(
            "kernel",
            format!("could not read the ARM64 Image header: {error}"),
        ));
    }
    if &header[56..60] != b"ARM\x64" {
        return Err(invalid_spec(
            "kernel",
            "expected an uncompressed ARM64 Linux Image",
        ));
    }
    let kernel_metadata = metadata("kernel", &kernel)?;
    let initramfs = spec
        .initramfs
        .as_ref()
        .map(|path| open_regular("initramfs", path, false))
        .transpose()?
        .as_ref()
        .map(|file| metadata("initramfs", file))
        .transpose()?;
    let root = disk("root_disk", &spec.root_disk.path, false)?;
    let data = disk("data_disk", &spec.data_disk.path, true)?;
    let read_only_inputs = [Some(&kernel_metadata), initramfs.as_ref(), Some(&root)];
    if read_only_inputs
        .into_iter()
        .flatten()
        .any(|input| same_file(input, &data))
        || data.nlink() != 1
    {
        return Err(invalid_spec(
            "data_disk",
            "the writable data disk must not alias other files",
        ));
    }
    let console_out = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&spec.console_log)
        .map_err(|error| invalid_file("console_log", error))?;
    let console = metadata("console_log", &console_out)?;
    // SAFETY: geteuid has no arguments and returns the effective user of this process.
    let owner = unsafe { libc::geteuid() };
    if console.uid() != owner
        || console.nlink() != 1
        || console.mode() & 0o077 != 0
        || same_file(&console, &data)
        || read_only_inputs
            .into_iter()
            .flatten()
            .any(|input| same_file(input, &console))
    {
        return Err(invalid_spec(
            "console_log",
            "the console log must be a private, distinct file",
        ));
    }
    Ok(Prepared {
        spec,
        console_out,
        discover_network: boot.interface.is_some(),
    })
}

fn prepare_shares(spec: &mut VmSpec) -> Result<Settings, VmError> {
    if spec
        .cmdline
        .split_ascii_whitespace()
        .any(|arg| arg.starts_with("marsa.share="))
    {
        return Err(invalid_spec(
            "cmdline",
            "guest mounts are constructed from the VM shares",
        ));
    }
    let mut arguments = String::new();
    for share in &spec.shares {
        append_share_argument(&mut arguments, share)?;
    }
    if !arguments.is_empty() {
        let tail = spec.cmdline.split_once(" -- ").map(|(boot, _)| boot.len());
        spec.cmdline
            .insert_str(tail.unwrap_or(spec.cmdline.len()), &arguments);
    }
    // The pinned arm64 kernel copies at most COMMAND_LINE_SIZE bytes, including NUL.
<<<<<<< HEAD
    if spec.cmdline.len() >= 2048 {
=======
    if spec.cmdline.len() >= ARM64_CMDLINE_BYTES {
>>>>>>> origin/main
        return Err(invalid_spec(
            "cmdline",
            "guest kernel arguments exceed 2047 bytes",
        ));
    }
    Settings::parse(&spec.cmdline).map_err(|error| invalid_file("cmdline", error))
}

fn append_share_argument(arguments: &mut String, share: &ShareSpec) -> Result<(), VmError> {
    let path = &share.host_path;
<<<<<<< HEAD
    let path_text = path
        .to_str()
        .ok_or_else(|| invalid_spec("shares.host_path", "expected a UTF-8 path"))?;
=======
>>>>>>> origin/main
    let canonical = path
        .canonicalize()
        .map_err(|error| invalid_file("shares.host_path", error))?;
    // macOS provides /tmp as an alias for /private/tmp. Keep its original guest mount path.
    let expected = path.strip_prefix("/tmp").map_or_else(
        |_| path.clone(),
        |suffix| Path::new("/private/tmp").join(suffix),
    );
    if canonical != expected
        || !fs::metadata(&canonical)
            .map_err(|error| invalid_file("shares.host_path", error))?
            .is_dir()
    {
        return Err(invalid_spec(
            "shares.host_path",
            "expected a directory without redirects",
        ));
    }

<<<<<<< HEAD
    arguments.push_str(" marsa.share=");
    arguments.push_str(&share.tag);
    arguments.push(':');
    for byte in path_text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._".contains(&byte) {
            arguments.push(char::from(byte));
        } else {
            write!(arguments, "%{byte:02X}")
                .map_err(|error| invalid_spec("shares.host_path", error.to_string()))?;
        }
    }
    Ok(())
=======
    append_share_arg(arguments, &share.tag, path)
        .map_err(|error| invalid_file("shares.host_path", error))
>>>>>>> origin/main
}

fn open_regular(field: &'static str, path: &Path, write: bool) -> Result<File, VmError> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| invalid_file(field, error))?;
    if metadata(field, &file)?.len() == 0 {
        return Err(invalid_spec(field, "expected a nonempty file"));
    }
    Ok(file)
}

fn disk(field: &'static str, path: &Path, write: bool) -> Result<Metadata, VmError> {
    let file = open_regular(field, path, write)?;
    let info = metadata(field, &file)?;
    if !info.len().is_multiple_of(512) {
        return Err(invalid_spec(
            field,
            "disk size must be a multiple of 512 bytes",
        ));
    }
    Ok(info)
}

fn metadata(field: &'static str, file: &File) -> Result<Metadata, VmError> {
    let info = file
        .metadata()
        .map_err(|error| invalid_file(field, error))?;
    if !info.is_file() {
        return Err(invalid_spec(field, "expected a regular file"));
    }
    Ok(info)
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

fn invalid_file(field: &'static str, error: io::Error) -> VmError {
    invalid_spec(field, error.to_string())
}
