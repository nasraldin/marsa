//! Collect bounded output from macOS policy commands and kill stalled children.

use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

pub(crate) async fn output(
    path: &str,
    arguments: &[&str],
    limit: Duration,
    max_bytes: u64,
) -> Option<Vec<u8>> {
    timeout(limit, async {
        let mut child = Command::new(path)
            .args(arguments)
            .env_clear()
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?.take(max_bytes + 1);
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).await.ok()?;
        if output.len() as u64 > max_bytes {
            return None;
        }
        child.wait().await.ok()?.success().then_some(output)
    })
    .await
    .ok()?
}
