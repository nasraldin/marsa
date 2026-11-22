# Marsa CLI exit codes

| Exit code | JSON error code | Meaning |
|---|---|---|
| 0 | | The command succeeded. Help and shell completion generation also return this status. |
| 1 | `not_available` | The command handler is awaiting implementation. A response write failure also exits with status 1. |
| 1 | `daemon_unavailable`, `agent_request_failed`, `agent_approval_missing`, `agent_grant_missing`, `mcp_unavailable` | The local daemon or MCP executable is unavailable, or it could not complete the agent command. |
| 2 | `invalid_invocation` | The command, its options or its arguments are invalid. |
| 3 | `confirmation_required` | A destructive command needs confirmation, or agent approval needs an interactive review. |
| 3 | `confirmation_declined` | The terminal review ended without an affirmative response. |

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

`marsa agents list --json` returns kind `agents`. Its `data` object contains `clients`, `grants` and `pending_requests` arrays. The client list shows unexpired local registrations. Each array is read from the daemon independently.

Successful approval, denial and revocation return kind `agent_decision` with `data.decision` and either `data.approval_id` or `data.grant_id`. Approval still requires terminal input and terminal standard error with `--json`. Standard output carries the JSON result.
