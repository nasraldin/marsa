//! Bounded standard-I/O protocol for the verified Docker credential helper.

use std::{collections::BTreeMap, io, path::Path, process::Stdio, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    time::timeout,
};

const KEYCHAIN_NOT_FOUND: &[u8] = b"credentials not found in native keychain";
const DEADLINE: Duration = Duration::from_secs(15);
const CREDENTIAL_LIMIT: usize = 8 * 1024;
const LIST_LIMIT: usize = 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Credential {
    #[serde(rename = "ServerURL")]
    pub(crate) server_url: String,
    pub(crate) username: String,
    pub(crate) secret: String,
}

pub(crate) async fn get(path: &Path, key: &str) -> io::Result<Option<Credential>> {
    let Some(output) = run(path, "get", key.as_bytes(), CREDENTIAL_LIMIT).await? else {
        return Ok(None);
    };
    let value = serde_json::from_slice::<Credential>(&output).map_err(|_| failure())?;
    if value.server_url != key || value.username.is_empty() {
        return Err(failure());
    }
    Ok(Some(value))
}

pub(crate) async fn list(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let output = run(path, "list", &[], LIST_LIMIT)
        .await?
        .ok_or_else(failure)?;
    serde_json::from_slice(&output).map_err(|_| failure())
}

pub(crate) async fn store(path: &Path, credential: &Credential) -> io::Result<()> {
    let input = serde_json::to_vec(credential).map_err(|_| failure())?;
    run(path, "store", &input, CREDENTIAL_LIMIT)
        .await?
        .ok_or_else(failure)?;
    Ok(())
}

pub(crate) async fn erase(path: &Path, key: &str) -> io::Result<()> {
    run(path, "erase", key.as_bytes(), CREDENTIAL_LIMIT).await?;
    Ok(())
}

async fn run(path: &Path, action: &str, input: &[u8], limit: usize) -> io::Result<Option<Vec<u8>>> {
    timeout(DEADLINE, async {
        let mut helper = Command::new(path)
            .arg(action)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        if let Some(mut stdin) = helper.stdin.take() {
            stdin.write_all(input).await?;
        }
        let mut output = Vec::new();
        helper
            .stdout
            .take()
            .ok_or_else(failure)?
            .take(limit as u64 + 1)
            .read_to_end(&mut output)
            .await?;
        if output.len() > limit {
            return Err(failure());
        }
        if helper.wait().await?.success() {
            return Ok(Some(output));
        }
        if output.trim_ascii() == KEYCHAIN_NOT_FOUND && matches!(action, "get" | "erase") {
            return Ok(None);
        }
        Err(failure())
    })
    .await
    .map_err(|_| io::ErrorKind::TimedOut)?
}

fn failure() -> io::Error {
    io::Error::other("Docker credential helper failed")
}
