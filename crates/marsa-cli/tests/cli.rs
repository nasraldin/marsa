//! Exercise the CLI's public command names, confirmation guard and outputs.

use std::process::{Command, Output};

fn marsa(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args(args)
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .expect("launch marsa")
}

#[test]
fn all_commands_have_help() {
    for path in [
        &[][..],
        &["status"],
        &["start"],
        &["stop"],
        &["restart"],
        &["env"],
        &["config"],
        &["config", "get"],
        &["config", "set"],
        &["config", "list"],
        &["share"],
        &["share", "list"],
        &["share", "approvals"],
        &["share", "resolve"],
        &["share", "add"],
        &["share", "remove"],
        &["disk"],
        &["disk", "usage"],
        &["disk", "cleanup"],
        &["disk", "reclaim"],
        &["network"],
        &["network", "status"],
        &["network", "mode"],
        &["logs"],
        &["diagnose"],
        &["update"],
        &["update", "check"],
        &["update", "apply"],
        &["mcp"],
        &["agents"],
        &["agents", "list"],
        &["agents", "approve"],
        &["agents", "deny"],
        &["agents", "revoke"],
        &["reset"],
        &["version"],
        &["completion"],
    ] {
        let result = marsa(&[path, &["--help"]].concat());
        assert!(result.status.success(), "{path:?}: {result:?}");
        let help = str::from_utf8(&result.stdout).unwrap();
        let invocation = [&["marsa"][..], path].concat().join(" ");
        assert!(help.contains(&invocation), "{path:?}: {help}");
    }
}

#[test]
fn destructive_commands_require_confirmation_on_a_pipe() {
    for (args, expected) in [
        (&["reset"][..], 3),
        (&["reset", "--factory"], 3),
        (&["reset", "--factory", "--help"], 0),
    ] {
        let result = marsa(args);
        assert_eq!(result.status.code(), Some(expected), "{args:?}: {result:?}");
    }
}

#[test]
fn all_supported_shells_get_the_full_command_tree() {
    for shell in ["bash", "zsh", "fish"] {
        let result = marsa(&["completion", shell]);
        assert!(result.status.success(), "{shell}: {result:?}");
        let script = str::from_utf8(&result.stdout).unwrap();
        for token in ["marsa", "reset", "dry-run", "reclaim", "agents", "config"] {
            assert!(script.contains(token), "{shell}: missing {token}");
        }
    }
}

#[test]
fn arabic_help_has_localized_headings() {
    let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args(["disk", "--help"])
        .env("LC_ALL", "ar_AE.UTF-8")
        .output()
        .unwrap();
    assert!(result.status.success());
    let help = str::from_utf8(&result.stdout).unwrap();
    assert!(help.contains("الاستخدام"), "{help}");
    assert!(help.contains("marsa disk"), "{help}");
}

#[test]
fn global_json_handles_help_and_errors_at_any_depth() {
    let cases: &[(&[&str], u8, &str, Option<&str>)] = &[
        (&["--json", "disk", "usage", "--help"], 0, "help", None),
        (&["disk", "usage", "--json", "--help"], 0, "help", None),
        (&["--json", "completion", "fish"], 0, "completion", None),
        (
            &["reset", "--json"],
            3,
            "error",
            Some("confirmation_required"),
        ),
        (
            &["share", "add", "--json"],
            2,
            "error",
            Some("invalid_invocation"),
        ),
        (
            &["--json", "update", "check"],
            1,
            "error",
            Some("not_available"),
        ),
        (
            &["network", "mode", "invalid", "--json"],
            2,
            "error",
            Some("invalid_invocation"),
        ),
        (
            &["--json", "completion", "invalid"],
            2,
            "error",
            Some("invalid_invocation"),
        ),
    ];
    for (args, code, kind, key) in cases {
        let result = marsa(args);
        assert_eq!(
            result.status.code(),
            Some(i32::from(*code)),
            "{args:?}: {result:?}"
        );
        assert!(result.stderr.is_empty(), "{args:?}: {result:?}");
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["schema_version"], 1, "{args:?}");
        assert_eq!(json["kind"], *kind, "{args:?}");
        if let Some(key) = key {
            assert_eq!(json["code"], *key, "{args:?}");
        }
    }
}

#[test]
#[cfg(unix)]
fn double_dash_keeps_json_literal_out_of_global_flags() {
    let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args(["config", "set", "key", "--", "--json"])
        .env("HOME", "/")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .expect("launch marsa");
    assert!(result.stdout.is_empty());
    assert_eq!(result.status.code(), Some(4));
    assert!(!result.stderr.is_empty());
}

#[test]
#[cfg(unix)]
fn authenticated_settings_share_and_network_commands_discover_the_daemon() {
    let commands: &[&[&str]] = &[
        &["config", "list"],
        &["share", "list"],
        &["share", "add", "/tmp/project"],
        &["share", "remove", "shared-folder-id"],
        &["share", "approvals"],
        &[
            "share",
            "resolve",
            "0123456789abcdef0123456789abcdef",
            "deny",
        ],
        &["network", "status"],
    ];
    for args in commands {
        let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
            .args(*args)
            .arg("--json")
            .env("HOME", "/")
            .output()
            .expect("launch marsa");
        assert_eq!(result.status.code(), Some(4), "{args:?}: {result:?}");
        let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(document["code"], "home_unavailable", "{args:?}");
    }
}

#[test]
#[cfg(unix)]
fn share_resolution_rejects_malformed_approval_ids() {
    let result = marsa(&["share", "resolve", "not-an-approval-id", "always", "--json"]);
    assert_eq!(result.status.code(), Some(2));
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["code"], "invalid_invocation");
}

#[test]
#[cfg(unix)]
fn env_quotes_the_socket_for_a_shell_and_reports_json_without_a_daemon() {
    let home = "/private/tmp/Marsa home 'quoted' $var";
    let command = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_marsa"))
            .args(args)
            .env("HOME", home)
            .env("LC_ALL", "en_US.UTF-8")
            .output()
            .unwrap()
    };
    let expected = format!("unix://{home}/.marsa/run/docker.sock");
    let json = command(&["env", "--json"]);
    assert!(json.status.success(), "{json:?}");
    let response: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(response["docker_host"], expected);

    let script = command(&["env"]);
    assert!(script.status.success(), "{script:?}");
    let mut script = String::from_utf8(script.stdout).unwrap();
    script.push_str("printf '%s' \"$DOCKER_HOST\"");
    let result = Command::new("sh").args(["-c", &script]).output().unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, expected.as_bytes());
}

#[test]
#[cfg(unix)]
fn disk_usage_does_not_report_success_when_the_daemon_cannot_be_reached() {
    let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args(["disk", "usage", "--json"])
        .env("HOME", "/")
        .output()
        .expect("launch marsa");
    assert_eq!(result.status.code(), Some(4));
    let body: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(body["code"], "home_unavailable");
}

#[test]
fn unsupported_cleanup_reports_nonzero() {
    for arguments in [
        &["disk", "cleanup", "--json"][..],
        &["disk", "cleanup", "--yes", "--json"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
            .args(arguments)
            .env("HOME", "/")
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        let body: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(body["code"], "not_available");
    }
}

#[test]
fn unavailable_message_preserves_localized_text_and_english_json() {
    for (locale, expected) in [
        ("en_US.UTF-8", "This command is not available yet.\n"),
        ("ar_AE.UTF-8", "هذا الأمر غير متاح حاليًا.\n"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_marsa"))
            .args(["update", "check"])
            .env("LC_ALL", locale)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, expected.as_bytes());
    }

    let output = Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args(["update", "check", "--json"])
        .env("LC_ALL", "ar_AE.UTF-8")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "schema_version": 1,
            "kind": "error",
            "code": "not_available",
            "message": "This command is not available yet.",
        }),
    );
}

#[test]
#[cfg(unix)]
fn disk_reclaim_reaches_the_authenticated_endpoint() {
    let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args(["disk", "reclaim", "--json"])
        .env("HOME", "/")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(4));
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["code"], "home_unavailable");
}

#[test]
#[cfg(unix)]
fn cleanup_preview_uses_the_authenticated_endpoint() {
    let result = Command::new(env!("CARGO_BIN_EXE_marsa"))
        .args([
            "disk",
            "cleanup",
            "--dry-run",
            "--include-volumes",
            "--json",
        ])
        .env("HOME", "/")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(4));
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["code"], "home_unavailable");
}

#[test]
fn agent_approval_requires_terminal_review_even_with_yes() {
    let result = marsa(&["agents", "approve", "fixture-id", "--yes", "--json"]);
    assert_eq!(result.status.code(), Some(3));
    assert!(result.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["code"], "confirmation_required");
}

#[test]
fn mcp_requires_a_client_and_project_and_reserves_stdout_for_protocol() {
    for args in [
        &["mcp"][..],
        &["mcp", "--client-id", "fixture"],
        &["mcp", "--client-id", "fixture", "--project", "relative"],
    ] {
        let result = marsa(args);
        assert_eq!(result.status.code(), Some(2), "{args:?}");
        assert!(result.stdout.is_empty());
    }
    let result = marsa(&[
        "mcp",
        "--client-id",
        "fixture",
        "--project",
        "/tmp",
        "--json",
    ]);
    // A configured launcher may print one JSON error for this rejected combination.
    assert_eq!(result.status.code(), Some(2));
}
