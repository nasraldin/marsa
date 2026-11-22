//! Classify log records without carrying any free-form source text into the archive.

use std::io;

use serde_json::{Value, json};

const MAX_LINE_BYTES: usize = 4096;
const MAX_RECORDS: usize = 256;

pub(super) fn report(
    input: &io::Result<Vec<u8>>,
    classify: impl Fn(&str) -> Option<Value>,
) -> Value {
    let bytes = match input {
        Ok(bytes) => bytes,
        Err(error) => {
            let status = if error.kind() == io::ErrorKind::NotFound {
                "missing"
            } else {
                "unavailable"
            };
            return json!({ "status": status, "records": [] });
        }
    };

    let mut considered = 0;
    let mut retained = 0;
    let mut records = Vec::new();
    for line in bytes
        .rsplit(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        considered += 1;
        let text = if line.len() <= MAX_LINE_BYTES {
            std::str::from_utf8(line).ok()
        } else {
            None
        };
        if let Some(record) = text.and_then(|line| classify(line.trim_ascii_end())) {
            retained += 1;
            if records.len() < MAX_RECORDS {
                records.push(record);
            }
        }
    }
    records.reverse();
    json!({
        "status": "summarized",
        "source_lines": considered,
        "matched": retained,
        "records": records,
    })
}

pub(super) fn daemon(line: &str) -> Option<Value> {
    let record: Value = serde_json::from_str(line).ok()?;
    let level = match record.get("level")?.as_str()? {
        "TRACE" => "trace",
        "DEBUG" => "debug",
        "INFO" => "info",
        "WARN" => "warn",
        "ERROR" => "error",
        _ => return None,
    };
    let event = match record.get("fields")?.get("message")?.as_str()? {
        "daemon started" => "daemon_started",
        "daemon stopped" => "daemon_stopped",
        "daemon shutdown requested" => "shutdown_requested",
        "engine shutdown failed" => "engine_shutdown_failed",
        "could not store settings" => "settings_write_failed",
        "settings directory sync failed" => "settings_directory_sync_failed",
        "automatic boot guest comparison unavailable" => "guest_comparison_unavailable",
        "automatic boot host comparison unavailable" => "host_comparison_unavailable",
        "automatic boot native egress check" => "native_egress_check",
        _ => "message_redacted",
    };
    Some(json!({ "event": event, "level": level }))
}

pub(super) fn console(line: &str) -> Option<Value> {
    if let Some((stage, duration)) = line
        .strip_prefix("marsa-init stage=")
        .and_then(|stage| stage.split_once(" duration_ms="))
    {
        let stage = [
            "mount_system",
            "data_disk",
            "shared_folders",
            "network",
            "agent",
            "containerd",
            "dockerd",
        ]
        .into_iter()
        .find(|label| *label == stage)?;
        let duration = duration.parse::<u32>().ok().filter(|ms| *ms <= 600_000)?;
        return Some(json!({ "event": "boot_stage", "stage": stage, "duration_ms": duration }));
    }

    let event = if line.starts_with("marsa-init: upstream interface ") {
        "upstream_interface_unavailable"
    } else if line.starts_with("marsa-init: replaying early kernel boot log") {
        "early_kernel_log"
    } else if line.starts_with("marsa-init:") {
        "init_message_redacted"
    } else {
        kernel_event(line)?
    };
    Some(json!({ "event": event }))
}

fn kernel_event(line: &str) -> Option<&'static str> {
    // Early dmesg replay can carry a priority prefix. The live console also
    // prepends a bracketed timestamp. Neither prefix enters the archive.
    let line = line
        .strip_prefix('<')
        .and_then(|prefix| prefix.split_once('>'))
        .map_or(line, |(_, message)| message);
    let line = line
        .strip_prefix('[')
        .and_then(|prefix| prefix.split_once(']'))
        .map_or(line, |(_, message)| message.trim_ascii_start());
    [
        ("Linux version ", "kernel_started"),
        ("Kernel command line:", "kernel_command_line_redacted"),
        ("Kernel panic - not syncing:", "kernel_panic"),
        ("VFS: Unable to mount root fs", "kernel_root_mount_failed"),
        (
            "printk: legacy console [hvc0] enabled",
            "virtio_console_enabled",
        ),
    ]
    .into_iter()
    .find_map(|(text, event)| line.starts_with(text).then_some(event))
}

pub(super) fn engine(line: &str) -> Option<Value> {
    let (level, message) = engine_fields(line)?;
    let level = match level {
        "trace" | "TRAC" | "TRACE" => "trace",
        "debug" | "DEBU" | "DEBUG" => "debug",
        "info" | "INFO" => "info",
        "warning" | "warn" | "WARN" => "warn",
        "error" | "ERRO" | "ERROR" => "error",
        "fatal" | "panic" | "FATA" | "FATAL" | "PANI" | "PANIC" => "fatal",
        _ => return None,
    };
    let message = message.trim_ascii_start();
    // The nonterminal format quotes startup messages with spaces.
    let message = message.strip_prefix('"').unwrap_or(message);
    let event = if message.starts_with("Docker daemon") {
        "docker_daemon"
    } else if message.starts_with("Daemon has completed initialization") {
        "docker_initialized"
    } else if message.starts_with("API listen on ") {
        "docker_api_listening"
    } else if message.starts_with("containerd successfully booted in ") {
        "containerd_ready"
    } else {
        "message_redacted"
    };
    Some(json!({ "event": event, "level": level }))
}

fn engine_fields(line: &str) -> Option<(&str, &str)> {
    // Logrus writes key-value fields to a pipe and a colored level followed
    // by a bracketed timestamp on a terminal such as the packaged console.
    if let Some(fields) = line.strip_prefix("time=\"") {
        let (_, fields) = fields.split_once("\" ")?;
        return fields.strip_prefix("level=")?.split_once(" msg=");
    }

    let line = without_color(line)?;
    let (level, timestamp) = line.split_at(line.find(['[', '\u{1b}'])?);
    let (timestamp, message) = without_color(timestamp)?
        .strip_prefix('[')?
        .split_once("] ")?;
    // Ignore kernel timestamps and other console text without this timestamp shape.
    if !timestamp.contains('T') {
        return None;
    }
    Some((level.trim_ascii_end(), message))
}

fn without_color(line: &str) -> Option<&str> {
    let Some(escape) = line.strip_prefix("\x1b[") else {
        return Some(line);
    };
    let (color, line) = escape.split_once('m')?;
    color
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b';')
        .then_some(line)
}

pub(super) fn network_reason(reason: Option<&str>) -> Option<&'static str> {
    match reason? {
        "probe_unavailable" => Some("probe_unavailable"),
        "transport_unavailable" => Some("transport_unavailable"),
        result if result.starts_with("native_") && result.ends_with("_userspace_verified") => {
            Some("userspace_verified")
        }
        result if result.starts_with("native_") && result.contains("_alternate_") => {
            Some("alternate_failed")
        }
        result if result.starts_with("native_") => Some("native_degraded"),
        _ => None,
    }
}
