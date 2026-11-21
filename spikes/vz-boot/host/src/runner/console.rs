//! Capture a bounded serial transcript and recognize complete guest status lines.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use super::Progress;

const MAX_CONSOLE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct SerialCapture {
    suffix: Vec<u8>,
    count: usize,
    closed: bool,
}

impl SerialCapture {
    pub(super) fn read(
        &mut self,
        stream: &mut UnixStream,
        transcript: &mut File,
        progress: &mut Progress,
    ) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let mut buffer = [0; 8192];
        let count = match stream.read(&mut buffer) {
            Ok(0) => {
                self.closed = true;
                return Ok(());
            }
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        self.count += count;
        if self.count > MAX_CONSOLE_BYTES {
            return Err(io::Error::other("guest console exceeded 16 MiB"));
        }
        let bytes = &buffer[..count];
        transcript.write_all(bytes)?;
        self.suffix.extend_from_slice(bytes);
        let seen = |marker: &[u8]| self.suffix.windows(marker.len()).any(|part| part == marker);
        progress.devices |= seen(b"MARSA_DEVICES_READY");
        progress.listening |= seen(b"MARSA_VSOCK_READY");
        progress.shell |= seen(b"MARSA_SHELL# ");
        if progress.commands_sent {
            for status in self
                .suffix
                .split_inclusive(|byte| *byte == b'\n')
                .filter_map(command_status)
            {
                if status != 0 {
                    return Err(io::Error::other(format!(
                        "guest script failed with exit status {status}; inspect the transcript"
                    )));
                }
                progress.commands_finished = true;
            }
        }
        let cut = self.suffix.len().saturating_sub(32);
        self.suffix.drain(..cut);
        Ok(())
    }
}

fn command_status(line: &[u8]) -> Option<u8> {
    let status = line.strip_prefix(b"MARSA_COMMANDS_")?.strip_suffix(b"\n")?;
    let status = status.strip_suffix(b"\r").unwrap_or(status);
    std::str::from_utf8(status).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::command_status;

    #[test]
    fn accepts_completed_console_status_lines() {
        assert_eq!(command_status(b"MARSA_COMMANDS_0\r\n"), Some(0));
        assert_eq!(command_status(b"MARSA_COMMANDS_124\n"), Some(124));
    }

    #[test]
    fn ignores_truncated_status_and_echoed_shell_input() {
        assert_eq!(command_status(b"MARSA_COMMANDS_1"), None);
        assert_eq!(command_status(b"MARSA_SHELL# MARSA_COMMANDS_0\r\n"), None);
        assert_eq!(command_status(b"MARSA_COMMANDS_%s\r\n"), None);
    }
}
