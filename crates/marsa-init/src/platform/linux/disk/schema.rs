//! Version the persistent data layout and adopt pristine ext4 volumes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use nix::fcntl::{self, AtFlags, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::linkat;

const SCHEMA_FILE: &str = "schema-version";
const SCHEMA_VERSION: u32 = 1;

/// Check the stored schema or initialize it on a pristine data volume.
pub(super) fn check_or_create(path: &Path) -> io::Result<()> {
    let schema = path.join(SCHEMA_FILE);
    match OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(&schema)
    {
        Ok(file) => {
            let mut version = Vec::new();
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "data disk schema must be a regular file",
                ));
            }
            file.take(12).read_to_end(&mut version)?;
            check_schema(&version)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => initialize_schema(path),
        Err(error) => Err(error),
    }
}

fn initialize_schema(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name() != "lost+found"
            || !entry.file_type()?.is_dir()
            || fs::read_dir(entry.path())?.next().is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "data disk has existing contents without a Marsa schema",
            ));
        }
    }
    // Publish a fully written marker without an observable empty file or a
    // leftover temporary path after a forced shutdown during initialization.
    let dir = File::open(path)?;
    let mut file = File::from(fcntl::open(
        path,
        OFlag::O_TMPFILE | OFlag::O_WRONLY | OFlag::O_CLOEXEC,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?);
    writeln!(file, "{SCHEMA_VERSION}")?;
    file.sync_all()?;
    linkat(&file, "", &dir, SCHEMA_FILE, AtFlags::AT_EMPTY_PATH)?;
    dir.sync_all()
}

fn check_schema(version: &[u8]) -> io::Result<()> {
    let valid_version = version
        .strip_suffix(b"\n")
        .filter(|digits| {
            digits
                .first()
                .is_some_and(|first| (b'1'..=b'9').contains(first))
        })
        .filter(|digits| digits.iter().all(u8::is_ascii_digit))
        .and_then(|digits| std::str::from_utf8(digits).ok())
        .and_then(|digits| digits.parse::<u32>().ok());
    match valid_version {
        Some(SCHEMA_VERSION) => Ok(()),
        Some(version) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "data disk schema version {version} is newer than supported version {SCHEMA_VERSION}"
            ),
        )),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid data disk schema version",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_future_and_invalid_schema_versions() {
        assert!(check_schema(b"1\n").is_ok());
        assert_eq!(
            check_schema(b"2\n").unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        for version in [b"".as_slice(), b"0\n", b"1\nother", b"+1\n"] {
            assert_eq!(
                check_schema(version).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
