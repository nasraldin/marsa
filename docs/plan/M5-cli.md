# M5 CLI

**Outcome:** the `marsa` command-line tool, a thin client of the typed API.

Design: [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli).

---

### M5-01 Typed API client library

- **Status:** done
- **Depends on:** M3-08
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api)
- **Do:** Implement `marsa-client`: connect to `marsad.sock`, start `marsad` if it is not running, typed wrappers for every service, an event subscription helper that delivers snapshot-then-stream, reconnection, and version checking against the daemon.
- **Deliver:** `crates/marsa-client` with tests against an in-process test daemon.
- **Accept when:**
  - A client started before the daemon connects once the daemon is up.
  - A daemon restart is survived by the event subscription without duplicate or lost state.
  - An incompatible daemon version yields a clear error.

### M5-02 CLI skeleton

- **Status:** done
- **Depends on:** M5-01
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Create the `marsa` binary with the full command tree, global `--json`, documented stable exit codes, `--help` for every command, shell completion generation, and the rule that destructive commands refuse to run non-interactively without `--yes`.
- **Deliver:** `crates/marsa-cli`, `docs/reference/cli-exit-codes.md`.
- **Accept when:**
  - Every command in the design exists and prints help.
  - `--json` output validates against checked-in JSON schemas.
  - Completions generate for zsh, bash and fish.

### M5-03 Engine commands

- **Status:** todo
- **Depends on:** M5-02
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `status`, `start`, `stop`, `restart`, `version`, `logs` (with `--follow`) and `env`.
- **Deliver:** command implementations and tests.
- **Accept when:**
  - `marsa start` returns when the engine is ready and shows progress while waiting.
  - `marsa status --json` includes VM state, versions and resource use.
  - `eval "$(marsa env)"` makes `docker ps` work in a shell with no context set.

### M5-04 Settings, sharing and network commands

- **Status:** todo
- **Depends on:** M5-02, M4-05, M4-10
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `config get|set|list`, `share list|add|remove` and `network status|mode`. `share add` answers pending bind-mount approvals from the terminal.
- **Deliver:** command implementations and tests.
- **Accept when:**
  - Invalid settings are rejected with the daemon's validation message.
  - Adding a protected path fails with an explanation.
  - A pending approval raised by `docker run -v` can be granted from the CLI.

### M5-05 Disk usage command

- **Status:** done
- **Depends on:** M5-02, M4-07
- **Docs:** [07 Data disk](../07-file-sharing-and-storage.md#data-disk)
- **Do:** Implement `marsa disk usage` using the authenticated `Disk.Usage` RPC. Report sparse host allocation and logical capacity separately. Report guest filesystem usage only when the daemon has a current sample.
- **Deliver:** the usage command, its JSON response schema, and tests.
- **Accept when:**
  - Human and JSON output expose host allocation and capacity with the VM stopped.
  - JSON distinguishes an unavailable guest sample from a real sample with zero bytes used.

### M5-06 Diagnostics and reset commands

- **Status:** todo
- **Depends on:** M5-02, M9-13
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `diagnose` and `reset` (settings only, and `--factory` with typed confirmation) as clients of the daemon calls delivered in M9-13 and M9-08. Until the reset service is available, report that status with a non-zero exit code.
- **Deliver:** command implementations.
- **Accept when:**
  - `reset --factory` cannot proceed without the typed confirmation or `--yes`.
  - `marsa diagnose` reports the path returned by M9-13. Verify diagnostic actions end to end in M9-07 and reset end to end in M9-08.

### M5-07 CLI localisation

- **Status:** todo
- **Depends on:** M5-03, M6-04
- **Docs:** [08 Internationalisation](../08-gui.md#internationalisation)
- **Do:** Localise human-readable CLI output in English and Arabic using the shared translation system, following the system language with an override flag and environment variable. Keep `--json`, command names and flags in English. Isolate technical values as left-to-right in Arabic output.
- **Deliver:** CLI strings in `locales/`, tests.
- **Accept when:**
  - Every CLI message has both translations (build fails otherwise).
  - Arabic output with embedded container names and paths reads correctly in a terminal that supports bidirectional text, and remains unambiguous in one that does not.


### M5-08 Preview and apply exact disk cleanup

- **Status:** todo
- **Depends on:** M5-05, M4-02
- **Docs:** [07 Disk cleanup with a preview](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
- **Do:** Implement a non-mutating `Disk.PrunePreview` and an authenticated cleanup RPC that applies an explicitly confirmed plan. Return the individual resources, grouped by category, and size estimates before confirmation. Bind apply to the preview's exact resource identities with a single-use, expiring plan and reject stale or ineligible resources. Use exact resource operations rather than broad prune calls. Keep volumes out of the default selection and include only explicitly selected eligible volumes. Implement `marsa disk cleanup --dry-run` and the confirmed apply path. A non-interactive apply requires the identifier returned by preview and `--yes`. `marsa disk reclaim` is owned by M9-03.
- **Deliver:** the preview and apply RPCs, the CLI flow, and tests with stale plans, concurrent changes, and volume selection.
- **Accept when:**
  - The preview lists exactly what the run then removes.
  - Volumes are removed only when named or explicitly included.
  - Applying a changed, expired, or reused plan cannot remove a resource outside the confirmed plan.
