//! Shared private staging and release tool execution for macOS delivery.

mod distribution;
mod signing;
mod verification;

use std::{
    env,
    ffi::OsString,
    fs::{self, File, Permissions},
    io::{self, Read},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde::{Deserialize, Serialize};

use crate::{release_resources::platform as staging, release_signature::valid_team_id};

pub(super) use distribution::package_release;
pub(super) use signing::{sign_local, sign_release};

const APP: &str = "Marsa.app";
const DMG: &str = "Marsa.dmg";
const RECEIPT: &str = "release-notarization.json";
const CODESIGN: &str = "/usr/bin/codesign";
const DITTO: &str = "/usr/bin/ditto";
const HDIUTIL: &str = "/usr/bin/hdiutil";
const XCRUN: &str = "/usr/bin/xcrun";
const IDENTITY: &str = "MARSA_SIGNING_IDENTITY";
const NOTARY_PROFILE: &str = "MARSA_NOTARY_PROFILE";
const TEAM_ID: &str = "MARSA_TEAM_ID";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    notarization_id: String,
    submitted_sha256: String,
    stapled_dmg_sha256: String,
    app_cdhash: String,
}

fn with_output(
    source: &Path,
    output: &Path,
    work: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let absolute = std::path::absolute(output)?;
    let name = absolute.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let parent_path = absolute.parent().ok_or(io::ErrorKind::InvalidInput)?;
    if parent_path.starts_with(source) {
        return Err(invalid("release output cannot be inside the input"));
    }
    let parent = staging::directory(parent_path)?;
    staging::owned_output_parent(&parent)?;
    staging::require_absent(&parent, name)?;
    staging::with_current_directory(&parent, || {
        let scratch = staging::Scratch::create()?;
        let work_path = std::path::absolute(scratch.path())?;
        let output = work_path.join("publish");
        fs::create_dir(&output)?;
        fs::set_permissions(&output, Permissions::from_mode(0o700))?;
        work(&work_path, &output)?;
        staging::directory(&output)?.sync_all()?;
        let temporary = staging::directory(&work_path)?;
        staging::publish_release(&temporary, &parent, parent_path, name)?;
        temporary
            .sync_all()
            .and_then(|()| parent.sync_all())
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("release was published but directory sync failed: {error}"),
                )
            })
    })
}

fn copy_bundle(source: &Path, target: &Path) -> io::Result<()> {
    checked(
        tool(DITTO).arg(source).arg(target),
        "could not stage the app",
    )?;
    Ok(())
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn valid_hash(hash: &str, length: usize) -> bool {
    hash.len() == length && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_receipt(input: File) -> io::Result<Receipt> {
    let mut bytes = Vec::new();
    input.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("the signing receipt exceeds its size limit"));
    }
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if !valid_uuid(&receipt.notarization_id)
        || !valid_hash(&receipt.submitted_sha256, 64)
        || !valid_hash(&receipt.stapled_dmg_sha256, 64)
        || !valid_hash(&receipt.app_cdhash, 40)
    {
        return Err(invalid("invalid digest in the signing receipt"));
    }
    Ok(receipt)
}

fn write_receipt(path: &Path, receipt: &Receipt) -> io::Result<()> {
    serde_json::to_writer_pretty(File::create(path)?, receipt).map_err(io::Error::other)
}

fn team() -> io::Result<String> {
    env::var(TEAM_ID)
        .ok()
        .filter(|team| valid_team_id(team))
        .ok_or_else(|| invalid("MARSA_TEAM_ID must contain a ten-character Apple Team ID"))
}

fn release_setting(name: &str) -> io::Result<OsString> {
    env::var_os(name)
        .filter(|setting| {
            setting.to_str().is_some_and(|value| {
                !value.trim().is_empty()
                    && !value.starts_with('-')
                    && !value.chars().any(char::is_control)
            })
        })
        .ok_or_else(|| invalid(format!("set a valid {name} in the release environment")))
}

fn tool(path: &str) -> Command {
    let mut command = Command::new(path);
    command.stdin(Stdio::null()).env("LC_ALL", "C");
    for setting in [IDENTITY, NOTARY_PROFILE, TEAM_ID] {
        command.env_remove(setting);
    }
    command
}

fn checked(command: &mut Command, failure: &str) -> io::Result<Output> {
    // Apple tools can include signing identity or the credential profile in
    // their output. Keep both streams private even when the command fails.
    let result = command
        .output()
        .map_err(|error| io::Error::new(error.kind(), failure))?;
    if result.status.success() {
        Ok(result)
    } else {
        Err(io::Error::other(format!("{failure} ({})", result.status)))
    }
}

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
