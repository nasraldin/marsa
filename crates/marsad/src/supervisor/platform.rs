//! Capture a small, sanitized span from the host's guest console file.

use std::path::PathBuf;

const CONSOLE_TAIL_BYTES: u64 = 8192;

pub(super) async fn console_tail(path: PathBuf) -> String {
    tokio::task::spawn_blocking(move || read_console(path).unwrap_or_default())
        .await
        .unwrap_or_default()
}

fn read_console(path: PathBuf) -> std::io::Result<String> {
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
    file.seek(SeekFrom::Start(
        metadata.len().saturating_sub(CONSOLE_TAIL_BYTES),
    ))?;
    let mut tail = Vec::new();
    file.take(CONSOLE_TAIL_BYTES).read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail)
        .chars()
        .filter(|&ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect())
}
