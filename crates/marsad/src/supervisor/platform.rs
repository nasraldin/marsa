//! Capture a small, sanitized span from the host's guest console file.

use std::{io, path::PathBuf};

use marsa_core::disk_schema::TooNew;

const CONSOLE_TAIL_BYTES: u64 = 8192;

pub(super) async fn console_tail(path: PathBuf) -> String {
    tokio::task::spawn_blocking(move || read_console(path, 0).unwrap_or_default())
        .await
        .unwrap_or_default()
}

pub(super) async fn console_position(path: &std::path::Path) -> u64 {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::symlink_metadata(path)
            .ok()
            .filter(|entry| entry.is_file())
            .map_or(0, |entry| entry.len())
    })
    .await
    .unwrap_or_default()
}

pub(super) async fn disk_schema_error(path: PathBuf, position: u64) -> Option<TooNew> {
    tokio::task::spawn_blocking(move || {
        let console = read_console(path, position).ok()?;
        console.lines().rev().find_map(TooNew::from_console_line)
    })
    .await
    .ok()
    .flatten()
}

fn read_console(path: PathBuf, position: u64) -> io::Result<String> {
    use std::{
        fs::OpenOptions,
        io::{Read, Seek, SeekFrom},
    };

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use nix::fcntl::OFlag;
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits());
    }

    let mut file = options.open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(String::new());
    }
    let earliest = position.max(metadata.len().saturating_sub(CONSOLE_TAIL_BYTES));
    file.seek(SeekFrom::Start(earliest))?;
    let mut tail = Vec::new();
    file.take(CONSOLE_TAIL_BYTES).read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail)
        .chars()
        .filter(|&ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[tokio::test]
    async fn rejects_schema_markers_from_previous_boots() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let marker = TooNew {
            found: 2,
            supported: 1,
        }
        .console_line();
        writeln!(file, "{marker}").unwrap();
        let previous = console_position(file.path()).await;
        writeln!(file, "current boot has a different failure").unwrap();
        assert_eq!(disk_schema_error(file.path().into(), previous).await, None);
        writeln!(file, "{marker}").unwrap();
        assert_eq!(
            disk_schema_error(file.path().into(), previous).await,
            Some(TooNew {
                found: 2,
                supported: 1
            })
        );
    }
}
