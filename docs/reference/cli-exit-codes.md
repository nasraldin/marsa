# Marsa CLI exit codes

| Exit code | JSON error code | Meaning |
|---|---|---|
| 0 | | The command succeeded. Help and shell completion generation also return this status. |
| 1 | `not_available`, `operation_failed` | A command is awaiting implementation, its operation failed, or a response could not be written. |
| 1 | `daemon_unavailable`, `agent_request_failed`, `agent_approval_missing`, `agent_grant_missing`, `mcp_unavailable` | The local daemon or MCP executable is unavailable, or it could not complete the agent command. |
| 2 | `invalid_invocation` | The command, its options or its arguments are invalid. |
| 3 | `confirmation_required` | A destructive command needs explicit confirmation. Non-interactive invocations require `--yes`. Agent approval requires an interactive review. |
| 3 | `confirmation_declined` | The terminal review ended without an affirmative response. |
| 4 | `connection_failed`, `home_unavailable`, `unprivileged_user_required` | The CLI could not establish an authenticated daemon connection or resolve the per-user socket. |
| 5 | `incompatible_daemon` | The daemon and CLI have incompatible API versions. |

In human-readable mode, help and completion scripts go to standard output.
Errors go to standard error. The command reads `LC_ALL`, `LC_MESSAGES` and
`LANG` in that order to select English or Arabic.

With `--json`, standard output carries one English JSON document conforming to
the [CLI response schema](cli-response.schema.json). Use `schema_version`,
`kind` and, for errors, `code` when processing a response. The `message`
and help `text` fields are for display.

```console
$ marsa reset --factory --json
{"code":"confirmation_required","kind":"error","message":"This operation requires --yes when run non-interactively.","schema_version":1}
```

The regular `disk cleanup`
command requires confirmation when run non-interactively. `disk cleanup
--dry-run` is a preview. When invoking shell completion with `--json`,
read the script from the response's `script` field.

`marsa disk usage --json` reports the sparse image's allocated bytes and
logical capacity under `host`. The `guest` field is `null` until the
daemon has a current filesystem observation. `categories` lists the
breakdown supplied by the daemon.

`marsa logs --follow --json` writes newline-delimited JSON, with one response
per log record. In the record, `message` contains a safe classification of
the source log and `at_unix_millis` is zero when wall-clock time is unavailable.
`marsa status --json` returns `resources: null` until the guest provides an
observation. The CPU field is cumulative VM CPU busy time in nanoseconds.

`marsa config list --json` and `marsa config get KEY --json` show the daemon
settings revision as a decimal string. `marsa config set KEY VALUE` accepts a
TOML value or a bare string. A rejected settings or shared-folder mutation
returns `request_rejected` with exit status 1 and the daemon's explanation.

`marsa network status --json` reports the configured and active network modes.
The guest and gateway fields are IPv4 strings when observed. When the VM is
stopped, the active mode and both addresses are `null`.

`marsa network mode --json` reports the configured and active network modes
and `restart_pending`. The value is true when the running guest uses an
earlier configured policy, false when the mode is current or the VM is
stopped, and `null` when the daemon has no boot-policy observation. A
successful explicit setting returns the same fields for the saved mode.

`marsa share approvals --json` shows exact requested directories and expiry
times. `marsa share resolve ID once|always|deny --json` reports whether the
owner needs to restart the VM before retrying Docker. A request that cannot be
resolved returns `bind_resolution_failed` with exit status 1.

`marsa diagnose --json` reports the daemon's local archive path in
`bundle_path`. A successful reset response reports `scope` as `settings`
or `factory`. With an older daemon that lacks reset support, the CLI reports
`not_available` after confirmation and exits with status 1.

`marsa disk reclaim --json` returns bytes the guest submitted for discard,
the observed decrease in allocated host bytes, and the disk usage after
the request. A stopped VM causes an operation error without booting it.

`marsa agents list --json` returns kind `agents`. Its `data` object contains `clients`, `grants` and `pending_requests` arrays. The client list shows unexpired local registrations. Each array is read from the daemon independently.

Successful approval, denial and revocation return kind `agent_decision` with `data.decision` and either `data.approval_id` or `data.grant_id`. Approval still requires terminal input and terminal standard error with `--json`. Standard output carries the JSON result.
