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
        (&["disk", "cleanup"], 3),
        (&["reset", "--factory", "--yes"], 1),
        (&["disk", "cleanup", "--yes"], 1),
        (&["disk", "cleanup", "--dry-run"], 1),
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
        (&["--json", "diagnose"], 1, "error", Some("not_available")),
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
fn double_dash_keeps_json_literal_out_of_global_flags() {
    let result = marsa(&["config", "set", "key", "--", "--json"]);
    assert!(result.stdout.is_empty());
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(result.stderr, b"This command is not available yet.\n");
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
