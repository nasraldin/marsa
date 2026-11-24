//! Constrained HTTPS process interface for macOS registry credential checks.

use std::{io, process::Stdio, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    time::timeout,
};

const CURL: &str = "/usr/bin/curl";
const RESPONSE_LIMIT: usize = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(15);

pub(crate) async fn get(
    url: &str,
    credential: Option<(&str, &str)>,
    service: Option<&str>,
) -> io::Result<String> {
    timeout(DEADLINE, async {
        let mut command = Command::new(CURL);
        command.args([
            "--disable",
            "--silent",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--noproxy",
            "*",
            "--http1.1",
            "--globoff",
            "--max-time",
            "12",
            "--max-filesize",
            "65536",
            "--include",
            "--config",
            "-",
        ]);
        if let Some(service) = service {
            command.args(["--get", "--data-urlencode", &format!("service={service}")]);
        }
        let mut child = command
            .args(["--url", url])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| failure())?;
        if let Some((username, secret)) = credential {
            let mut stdin = child.stdin.take().ok_or_else(failure)?;
            let token = STANDARD.encode(format!("{username}:{secret}"));
            let input = format!("header = \"Authorization: Basic {token}\"\n");
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(|_| failure())?;
        } else {
            drop(child.stdin.take());
        }
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(failure)?
            .take(RESPONSE_LIMIT as u64 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| failure())?;
        if output.len() > RESPONSE_LIMIT || !child.wait().await.map_err(|_| failure())?.success() {
            return Err(failure());
        }
        String::from_utf8(output).map_err(|_| failure())
    })
    .await
    .map_err(|_| failure())?
}

fn failure() -> io::Error {
    io::Error::other("HTTPS registry request failed")
}
