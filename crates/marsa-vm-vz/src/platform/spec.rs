//! Resolve host files before asking the framework to consume the configuration.

use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use marsa_vm::{VmError, VmSpec};

use super::state::{invalid_spec, unsupported};

const MIB: u64 = 1024 * 1024;

pub(super) struct Prepared {
    pub(super) spec: VmSpec,
    pub(super) console_out: File,
}

pub(super) fn prepare(spec: VmSpec) -> Result<Prepared, VmError> {
    spec.validate()?;
    if !spec.memory_bytes.is_multiple_of(MIB) {
        return Err(invalid_spec(
            "memory_bytes",
            "memory must be a multiple of 1 MiB",
        ));
    }
    if !spec.shares.is_empty() {
        return Err(unsupported("VZ shared folders"));
    }

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
    Ok(Prepared { spec, console_out })
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
