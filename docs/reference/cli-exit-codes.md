# Marsa CLI exit codes

| Exit code | JSON error code | Meaning |
|---|---|---|
| 0 | | The command succeeded. Help and shell completion generation also return this status. |
| 1 | `not_available`, `operation_failed` | A command is awaiting implementation, its operation failed, or a response could not be written. |
| 2 | `invalid_invocation` | The command, its options or its arguments are invalid. |
| 3 | `confirmation_required` | A destructive command needs the explicit `--yes` flag in a non-interactive invocation. |
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

JSON mode uses non-interactive confirmation rules. The regular `disk cleanup`
command requires confirmation when run non-interactively. `disk cleanup
--dry-run` is a preview. When invoking shell completion with `--json`,
read the script from the response's `script` field.

`marsa logs --follow --json` writes newline-delimited JSON, with one response
per log record. In the record, `message` contains a safe classification of
the source log and `at_unix_millis` is zero when wall-clock time is unavailable.
`marsa status --json` returns `resources: null` until the guest provides an
observation. The CPU field is cumulative VM CPU busy time in nanoseconds.

`marsa share approvals --json` shows exact requested directories and expiry
times. `marsa share resolve ID once|always|deny --json` reports whether the
owner needs to restart the VM before retrying Docker. A request that cannot be
resolved returns `bind_resolution_failed` with exit status 1.
