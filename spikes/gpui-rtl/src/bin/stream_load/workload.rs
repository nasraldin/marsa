//! Sustain a bounded queue and log history throughout the load probe.

use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use gpui_kit::SharedString;

pub const RATE: usize = 10_000;
pub const BATCH_LINES: usize = 100;
pub const QUEUED_BATCHES: usize = 4;
pub const LINE_CAP: usize = 10_000;
pub const BYTE_CAP: usize = 1024 * 1024;

const INTERVAL: Duration = Duration::from_millis(10);

pub struct LogLine {
    pub sequence: usize,
    pub text: SharedString,
}

#[derive(Default)]
pub struct History {
    pub lines: VecDeque<LogLine>,
    pub accepted: usize,
    pub peak_lines: usize,
    pub peak_bytes: usize,
    bytes: usize,
}

impl History {
    pub fn extend(&mut self, batch: Vec<LogLine>) {
        for line in batch {
            let bytes = line.text.len();
            if bytes > BYTE_CAP {
                continue;
            }
            while self.lines.len() == LINE_CAP || self.bytes + bytes > BYTE_CAP {
                let Some(oldest) = self.lines.pop_front() else {
                    break;
                };
                self.bytes -= oldest.text.len();
            }
            self.bytes += bytes;
            self.accepted += 1;
            self.lines.push_back(line);
            self.peak_lines = self.peak_lines.max(self.lines.len());
            self.peak_bytes = self.peak_bytes.max(self.bytes);
        }
    }
}

pub fn source(seconds: usize) -> io::Result<Receiver<Vec<LogLine>>> {
    let (sender, receiver) = sync_channel(QUEUED_BATCHES);
    thread::Builder::new()
        .name("spike-fixture-log-producer".into())
        .spawn(move || produce(sender, seconds * RATE / BATCH_LINES))?;
    Ok(receiver)
}

fn produce(sender: SyncSender<Vec<LogLine>>, batches: usize) {
    let mut deadline = Instant::now() + INTERVAL;
    for batch in 0..batches {
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
        deadline += INTERVAL;
        let lines = (0..BATCH_LINES)
            .map(|index| {
                let sequence = batch * BATCH_LINES + index + 1;
                LogLine {
                    sequence,
                    text: format!(
                        "{sequence:08} INFO service=container-{:04} request={:08x} method=GET status=200 duration_ms={}",
                        sequence % 2000,
                        sequence,
                        sequence % 250
                    )
                    .into(),
                }
            })
            .collect();
        if sender.send(lines).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_oversized_line_preserves_valid_history() {
        let mut history = History::default();
        history.extend(vec![
            LogLine {
                sequence: 1,
                text: "first".into(),
            },
            LogLine {
                sequence: 2,
                text: "x".repeat(BYTE_CAP + 1).into(),
            },
        ]);
        assert_eq!(history.lines.front().map(|line| line.sequence), Some(1));
        assert_eq!(history.accepted, 1);
    }
}
