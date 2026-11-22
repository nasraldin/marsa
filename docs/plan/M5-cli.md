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

- **Status:** done
- **Depends on:** M5-02, M4-16, M9-13
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `status`, `start`, `stop`, `restart`, `version`, `logs` (with `--follow`) and `env`. Complete the diagnostic service’s bounded, sanitized log stream.
- **Deliver:** command implementations and tests.
- **Accept when:**
  - `marsa start` returns when the engine is ready and shows progress while waiting.
  - `marsa status --json` includes VM state, versions and resource use.
  - `eval "$(marsa env)"` makes `docker ps` work in a shell with no context set.

### M5-04 Network mode and settings

- **Status:** todo
- **Depends on:** M5-02, M4-10
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `network mode` through the authenticated network API. Report when a change needs a restart. M5-10 owns settings and shared folders, M5-09 owns bind approvals, and M5-11 owns read-only network status.
- **Deliver:** network mode command and tests.
- **Accept when:**
  - An explicit mode stages only the requested configuration and reports the restart requirement.

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
- **Do:** Implement `diagnose` and `reset` (settings only, and `--factory` with typed confirmation) as clients of the daemon calls delivered in M9-13, M9-17 and M9-18. Until the reset service is available, report that status with a non-zero exit code.
- **Deliver:** command implementations.
- **Accept when:**
  - `reset --factory` cannot proceed without the typed confirmation or `--yes`.
  - `marsa diagnose` reports the path returned by M9-13. Verify diagnostic actions end to end in M9-07 and settings reset in M9-17 and factory reset in M9-18.

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

### M5-09 Review and answer bind approvals from the CLI

- **Status:** done
- **Depends on:** M5-02, M4-06
- **Docs:** [07 The allowlist](../07-file-sharing-and-storage.md#the-allowlist), [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** List current typed bind-approval events, show the exact requested directories, and submit once, always or deny by approval ID. Explain when a VM restart is needed. Provide English and Arabic display strings and stable JSON.
- **Deliver:** CLI commands and a signed integration test covering authorization from the real Docker CLI.
- **Accept when:**
  - A request from `docker run -v` can be reviewed and granted from a separate terminal, and the mount works after explicit application.
  - An expired or mismatched approval has no effect on the configured shares.

### M5-10 Settings and shared-folder CLI commands

- **Status:** done
- **Depends on:** M5-02, M4-05
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `config get|set|list` and `share list|add|remove` through the authenticated typed client. Use the current revision for settings updates and expose the daemon's validation and restart results.
- **Deliver:** command implementations and tests in English, Arabic, and JSON.
- **Accept when:**
  - The daemon's validation message explains an invalid setting.
  - Adding a protected path fails safely with an explanation.
  - The owner can list, add and remove an ordinary folder without changing other settings.

### M5-11 Report active networking from the CLI

- **Status:** done
- **Depends on:** M5-02, M4-18
- **Docs:** [06 Mode selection](../06-networking.md#network-modes), [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Report configured and active network modes, connectivity reason, and current addresses from the authenticated read-only network RPC. Localize the human-readable output and keep JSON stable.
- **Deliver:** `marsa network status` and a packaged verification.
- **Accept when:**
  - A stopped daemon reports the configured mode without an active mode or stale addresses.
  - A running user-space guest reports its active mode and IPv4 and gateway values.
