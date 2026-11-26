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

### M5-04 Configure the network mode from the CLI

- **Status:** done
- **Depends on:** M5-03, M5-11, M4-25
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `network mode [auto|nat|userspace]` through the authenticated network API. Report configured and active modes separately and show whether a restart is pending.
- **Deliver:** network mode command and tests.
- **Accept when:**
  - An explicit mode stages only the requested configuration and reports the restart requirement.
- **Evidence:** The signed fixture saved `auto` while the existing user-space guest stayed Ready with the same helper process. The CLI reported pending before an explicit stop and the saved mode with no active attachment after it.

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

- **Status:** done
- **Depends on:** M5-02, M9-13, M9-17
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `diagnose` and `reset` (settings only, and `--factory` with typed confirmation) as clients of the daemon calls delivered in M9-13, M9-17 and M9-18. Until the reset service is available, report that status with a non-zero exit code.
- **Deliver:** command implementations.
- **Accept when:**
  - `reset --factory` cannot proceed without the typed confirmation or `--yes`.
  - `marsa diagnose` reports the path returned by M9-13. Verify the settings reset using the daemon's separate prepare and confirm actions. M9-07 owns the GUI diagnostic flow and M9-18 retains factory-reset acceptance.
- **Evidence:** The signed CLI and daemon wrote a mode-0600 archive locally with the VM stopped. A settings reset without confirmation left the fixture configuration intact. The confirmed request restored defaults through the settings challenge. The factory request reported unavailable, and the fixture disk kept its original inode and contents.

A signed CLI and daemon in a disposable home also exercised the factory service from M9-18. The CLI returned exit 3 without confirmation, preserved the original disk and grants, and then returned a successful reset after `--yes`. The new disk was sparse, grants were cleared, defaults persisted and an unrelated state entry was unchanged.

### M5-07 English command-line output

- **Status:** done
- **Depends on:** M5-03, M6-12
- **Docs:** [CLI output](../reference/cli-exit-codes.md), [08 Internationalisation](../08-gui.md#internationalisation)
- **Do:** Keep Marsa's CLI help, messages, confirmation prompts and completion descriptions in English regardless of the environment. The owner clarified that English and Arabic language selection applies to the desktop GUI.
- **Deliver:** English CLI output, typed settings and shared-folder validation feedback, and regression tests.
- **Accept when:**
  - Arabic locale settings and legacy environment overrides leave CLI output in English. Remove the command-line language option.
  - Preserve JSON contracts, exact confirmation phrases, control escaping for terminal display and raw redirected data.
- **Evidence:** CLI regression tests compare English and Arabic shell environments for root and nested help, JSON help, completion output and errors. Private-home executable fixtures verified English MCP startup diagnostics and tool descriptions, reserved protocol stdout, and exact terminal approval with `yes`. The shared Fluent suite still tests Arabic plural forms.

### M5-08 Preview unused guest storage

- **Status:** done
- **Depends on:** M5-05, M4-02
- **Docs:** [07 Disk cleanup with a preview](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
- **Do:** Implement owner-authenticated `Disk.PrunePreview` and `marsa disk cleanup --dry-run` using the ready engine's read-only disk usage endpoint. Report individual observed resources by category, known guest byte estimates and unavailable information. Exclude volumes by default and list only eligible local volumes on explicit selection. A preview never starts the VM.
- **Deliver:** Read-only RPC and CLI preview with fixtures.
- **Accept when:**
  - A running engine lists stopped containers, unused images and available unused cache rows with their observed estimates. Explicit volume selection lists only unused eligible volumes.
  - A stopped engine remains stopped. Missing category records and unavailable sizes are visible in output.
  - Only preview is available. The command rejects apply before contacting the daemon, even with `--yes`.

### M5-09 Review and answer bind approvals from the CLI

- **Status:** done
- **Depends on:** M5-02, M4-06
- **Docs:** [07 The allowlist](../07-file-sharing-and-storage.md#the-allowlist), [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** List current typed bind-approval events, show the exact requested directories, and submit once, always or deny by approval ID. Explain when a VM restart is needed. Use English CLI messages and stable JSON under every shell locale (D-033).
- **Deliver:** CLI commands and a signed integration test covering authorization from the real Docker CLI.
- **Accept when:**
  - A request from `docker run -v` can be reviewed and granted from a separate terminal, and the mount works after explicit application.
  - An expired or mismatched approval has no effect on the configured shares.

### M5-10 Settings and shared-folder CLI commands

- **Status:** done
- **Depends on:** M5-02, M4-05
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Implement `config get|set|list` and `share list|add|remove` through the authenticated typed client. Use the current revision for settings updates and expose the daemon's validation and restart results.
- **Deliver:** command implementations and tests for English messages and stable JSON under English and Arabic shell locales (D-033).
- **Accept when:**
  - The daemon's validation message explains an invalid setting.
  - Adding a protected path fails safely with an explanation.
  - The owner can list, add and remove an ordinary folder without changing other settings.

### M5-11 Report active networking from the CLI

- **Status:** done
- **Depends on:** M5-02, M4-18
- **Docs:** [06 Mode selection](../06-networking.md#network-modes), [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli)
- **Do:** Report configured and active network modes, connectivity reason, and current addresses from the authenticated read-only network RPC. Use English for human-readable output under every shell locale and keep JSON stable (D-033).
- **Deliver:** `marsa network status` and a packaged verification.
- **Accept when:**
  - A stopped daemon reports the configured mode without an active mode or stale addresses.
  - A running user-space guest reports its active mode and IPv4 and gateway values.

### M5-12 Apply an exactly confirmed cleanup plan

- **Status:** blocked (Docker 29.10.0 lacks conditional exact-identity deletion)
- **Depends on:** M5-08
- **Docs:** [07 Disk cleanup with a preview](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
- **Do:** Add authenticated apply after a supported engine interface can guarantee immutable identity and eligibility at deletion. Bind each apply to the exact resources selected by an expiring, single-use preview. Use exact operations without forcing or broad prune. Keep named volumes excluded by default and include only explicitly selected eligible volumes. A non-interactive apply requires the identifier returned by preview and `--yes`.
- **Deliver:** Exact apply RPC and CLI confirmation, tested under concurrent resource replacement.
- **Accept when:**
  - The preview lists exactly what apply removes.
  - Volumes are removed only when named or explicitly included.
  - Applying a changed, expired or reused plan cannot remove a resource outside the confirmed plan.

Resume when a supported engine API can reject a mutable name or stale target
at deletion. The [pinned Docker source](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
documents the current boundary.
M7-21 uses this same capability gate for desktop cleanup review.
