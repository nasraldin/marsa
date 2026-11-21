//! Capture a bounded serial transcript and recognize complete guest status lines.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use super::Progress;

const MAX_CONSOLE_BYTES: usize = 16 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 4096;
const PROMPT: &[u8] = b"MARSA_SHELL# ";

#[derive(Default)]
pub(super) struct SerialCapture {
    line: Vec<u8>,
    discarding_line: bool,
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
        self.accept(bytes, progress)
    }

    fn accept(&mut self, bytes: &[u8], progress: &mut Progress) -> io::Result<()> {
        for fragment in bytes.split_inclusive(|byte| *byte == b'\n') {
            let complete = fragment.ends_with(b"\n");
            if self.discarding_line || self.line.len() + fragment.len() > MAX_LINE_BYTES {
                self.discarding_line = !complete;
                self.line.clear();
                continue;
            }

            self.line.extend_from_slice(fragment);
            if !progress.commands_sent && self.line.starts_with(PROMPT) {
                progress.shell = true;
            }
            if complete {
                accept_line(&self.line, progress)?;
                self.line.clear();
            }
        }
        Ok(())
    }
}

fn accept_line(line: &[u8], progress: &mut Progress) -> io::Result<()> {
    match line {
        b"MARSA_DEVICES_READY\n" | b"MARSA_DEVICES_READY\r\n" => progress.devices = true,
        b"MARSA_VSOCK_READY\n" | b"MARSA_VSOCK_READY\r\n" => progress.listening = true,
        _ if progress.commands_sent => match command_status(line) {
            Some(0) => progress.commands_finished = true,
            Some(status) => {
                return Err(io::Error::other(format!(
                    "guest script failed with exit status {status}; inspect the transcript"
                )));
            }
            None => {}
        },
        _ => {}
    }
    Ok(())
}

fn command_status(line: &[u8]) -> Option<u8> {
    let status = line.strip_prefix(b"MARSA_COMMANDS_")?.strip_suffix(b"\n")?;
    let status = status.strip_suffix(b"\r").unwrap_or(status);
    if status.is_empty() || status.len() > 3 || !status.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(status).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{Progress, SerialCapture, command_status};

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
        assert_eq!(command_status(b"MARSA_COMMANDS_+0\r\n"), None);
    }

    #[test]
    fn accepts_markers_split_across_reads() {
        let mut capture = SerialCapture::default();
        let mut progress = Progress::default();
        for bytes in [
            b"MARSA_DEVICES_REA".as_slice(),
            b"DY\r",
            b"\nMARSA_VSOCK_READY\nMARSA_SHE",
            b"LL# ",
        ] {
            capture.accept(bytes, &mut progress).unwrap();
        }
        assert!(progress.devices && progress.listening && progress.shell);
        progress.commands_sent = true;
        capture
            .accept(b"command echo\r\nMARSA_COMMANDS_", &mut progress)
            .unwrap();
        assert!(!progress.commands_finished);
        capture.accept(b"0\r\n", &mut progress).unwrap();
        assert!(progress.commands_finished);
    }

    #[test]
    fn ignores_ready_markers_inside_diagnostic_lines() {
        let mut capture = SerialCapture::default();
        let mut progress = Progress::default();
        capture
            .accept(
                b"diagnostic MARSA_DEVICES_READY\r\nlog MARSA_VSOCK_READY\nlog MARSA_SHELL# ",
                &mut progress,
            )
            .unwrap();
        assert!(!progress.devices && !progress.listening && !progress.shell);
    }

    #[test]
    fn drops_overlong_records_until_the_next_newline() {
        let mut capture = SerialCapture::default();
        let mut progress = Progress {
            commands_sent: true,
            ..Progress::default()
        };
        let mut prefix = vec![b'x'; 8192];
        prefix.extend(b"MARSA_COMMANDS_0");
        capture.accept(&prefix, &mut progress).unwrap();
        assert!(!progress.commands_finished);
        capture.accept(b"\r\n", &mut progress).unwrap();
        assert!(!progress.commands_finished);
        capture
            .accept(b"MARSA_COMMANDS_0\n", &mut progress)
            .unwrap();
        assert!(progress.commands_finished);
    }

    #[test]
    fn ignores_stale_status_after_sending_commands() {
        let mut capture = SerialCapture::default();
        let mut progress = Progress::default();
        let mut diagnostic = vec![b'x'; 8192];
        diagnostic.extend(b"MARSA_COMMANDS_0\r\n");
        diagnostic.extend([b'x'; 14]);
        capture.accept(&diagnostic, &mut progress).unwrap();
        progress.commands_sent = true;
        capture.accept(b"\r\n", &mut progress).unwrap();
        assert!(!progress.commands_finished);
    }

    #[test]
    fn a_failed_command_cannot_be_reported_as_complete() {
        let mut capture = SerialCapture::default();
        let mut progress = Progress {
            commands_sent: true,
            ..Progress::default()
        };
        capture.accept(b"MARSA_COMMANDS_2", &mut progress).unwrap();
        assert!(capture.accept(b"\r\n", &mut progress).is_err());
        assert!(!progress.commands_finished);
    }
}
