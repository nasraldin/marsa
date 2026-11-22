# M9 Hardening

**Outcome:** security, reliability and recovery are proven by tests, not assumed.

Design: [11 Security](../11-security.md), [13 Testing and benchmarks](../13-testing-and-benchmarks.md), [07 File sharing and storage](../07-file-sharing-and-storage.md).

---

### M9-01 Security test suite

- **Status:** todo
- **Depends on:** M4-14
- **Docs:** [13 Security tests](../13-testing-and-benchmarks.md#security-tests), [11 Threat model](../11-security.md#threat-model)
- **Do:** Implement the automated security tests: socket and directory permissions, access by another user, allowlist escapes (symlink, `..`, case variants, hard links), protected paths, loopback-only port binding, no TCP listener for the engine, and log redaction.
- **Deliver:** `tests/security/`, wired into CI.
- **Accept when:**
  - Each threat T1, T2, T9 and T11 in the threat model has at least one test that would fail if the mitigation were removed.

### M9-02 Fuzz targets

- **Status:** done
- **Depends on:** M3-07, M10-03
- **Docs:** [11 Process hardening on the host](../11-security.md#process-hardening-on-the-host)
- **Do:** Add fuzz targets for the guest control-protocol decoder, the settings parser and the update manifest parser. Run them on a schedule in CI with a corpus kept in the repository. M9-15 adds the Docker create-request inspector after M4-06 implements it.
- **Deliver:** `fuzz/` targets and a scheduled workflow.
- **Accept when:**
  - Each target runs for a fixed budget in CI without crashes.
  - Any crash found is fixed and its input added as a regression test.

### M9-03 Disk space reclaim end to end

- **Status:** todo
- **Depends on:** M4-07, M2-08, M1-03
- **Docs:** [07 Returning space to macOS](../07-file-sharing-and-storage.md#returning-space-to-macos), V-05, R-14
- **Do:** Implement `Disk.Reclaim` using the mechanism proven in M1-03, automatic reclaim after cleanup and on a schedule when idle, and the offline compaction fallback if it was needed. Wire `marsa disk reclaim` and the GUI cleanup flow.
- **Deliver:** reclaim implementation and a Tier 2 test.
- **Accept when:**
  - A test that pulls several gigabytes of images, removes them and reclaims shows the image file's real size on the host dropping by at least 90% of the freed amount.
  - Reclaim never runs during heavy I/O without the user asking.

### M9-04 Memory behaviour

- **Status:** todo
- **Depends on:** M4-02, M3-04
- **Docs:** [04 Memory management](../04-virtualization-macos.md#memory-management), V-04, R-03
- **Do:** Measure how much memory the balloon actually returns to macOS under `vz` (V-04) and record it. Implement the memory policy: right-sized default, balloon on idle if it helps, idle stop as the dependable mechanism, all configurable.
- **Deliver:** memory policy in the supervisor, a measurement report in `docs/spikes/M9-04-memory.md`.
- **Accept when:**
  - The report states measured host memory before, during and after a 2 GiB workload, with and without ballooning.
  - With idle stop on, host memory returns to the daemon's idle footprint after the timeout.
  - V-04 is resolved in the docs.

### M9-05 Sleep, wake and network-change robustness

- **Status:** todo
- **Depends on:** M3-11, M4-10
- **Docs:** [06 Sleep, wake and network change](../06-networking.md#sleep-wake-and-network-change), [03 Sleep, wake and network changes](../03-host-daemon.md#sleep-wake-and-network-changes)
- **Do:** Make the whole stack correct across sleep, wake and network changes: time resync, DNS and proxy refresh, mode re-probe, listener validation, closing dead connections. Add tests with injected events and a real-hardware test procedure.
- **Deliver:** fixes and tests.
- **Accept when:**
  - After an hour of sleep, published ports and DNS work without a restart.
  - Switching between two networks restores connectivity for running containers.

### M9-06 Crash recovery

- **Status:** done
- **Depends on:** M3-06, M4-07
- **Docs:** [04 Failure handling](../04-virtualization-macos.md#failure-handling)
- **Do:** Recover from daemon death without leaving a VM or user-space helper alive, replace stale runtime sockets on start and preserve the data disk. M9-16 verifies stale published port listeners after M4-09 implements forwarding.
- **Deliver:** recovery logic and fault-injection tests.
- **Accept when:**
  - Killing `marsad` at each supervisor state and restarting always reaches a correct state without manual steps.
  - A named volume written before the kill is intact afterwards.

**Verification.** Private subprocesses were killed at all eight supervisor states. Each replacement served the Unix API and rebound the Docker socket. The signed Apple silicon test killed the daemon in `Ready`, observed the old network helper exit and verified that the replacement used the original disk and named volume. A later clean shutdown left no ext4 recovery flag.

### M9-16 Recover published ports after a daemon crash

- **Status:** todo
- **Depends on:** M4-09, M9-06
- **Docs:** [06 Published ports](../06-networking.md#published-ports), [04 Failure handling](../04-virtualization-macos.md#failure-handling)
- **Do:** Verify recovery of published TCP and UDP listeners when the daemon is killed, including any helper that owns a listener.
- **Deliver:** Production port recovery changes where needed and a crash-injection regression in the port-forwarding suite.
- **Accept when:**
  - The stale published listeners close after daemon death and a replacement can bind the same endpoints without manual cleanup.
  - Recovery preserves the configured exposure policy and leaves an unrelated listener alone.

### M9-07 Diagnostics bundle actions

- **Status:** todo
- **Depends on:** M5-06, M7-09, M9-13
- **Docs:** [09 The marsa CLI](../09-cli-and-docker-compat.md#the-marsa-cli), [11 Threat model](../11-security.md#threat-model) (T11)
- **Do:** Connect the CLI and GUI diagnostic actions to the daemon collector. Show the locally created archive path and let the user review it before sharing.
- **Deliver:** CLI and GUI integration and end-to-end tests for the diagnostic actions.
- **Accept when:**
  - `marsa diagnose` and the GUI action both produce the bundle and show its path.
  - Neither action uploads the bundle.

### M9-08 Factory reset and clean uninstall

- **Status:** todo
- **Depends on:** M4-07, M4-04
- **Docs:** [07 Integrity](../07-file-sharing-and-storage.md#integrity), [12 Uninstall](../12-build-release-update.md#uninstall)
- **Do:** Implement `Engine.Reset` (settings only, or factory: delete and recreate the data disk, clear grants) behind a confirmation token. Implement and document the uninstall procedure, and an automated check that nothing is left behind.
- **Deliver:** reset implementation, uninstall script or command, a clean-machine test.
- **Accept when:**
  - Factory reset works even when the engine cannot start.
  - After uninstall, no Marsa file, launch agent, Docker context, symlink or PATH entry remains (verified by the test).

### M9-09 Data-disk versioning and upgrade path

- **Status:** done
- **Depends on:** M4-07
- **Docs:** [05 Versioning and upgrades](../05-guest-image.md#versioning-and-upgrades)
- **Do:** Add the data-disk schema version, the refusal to start on a newer schema with a clear host-side message, and the upgrade test: a data disk produced by the previous release must work with the new guest image.
- **Deliver:** versioning logic and an upgrade test that keeps a fixture disk from each release.
- **Accept when:**
  - The upgrade test passes with images, containers and volumes intact.
  - A downgrade is detected and explained without touching the data.

### M9-10 Soak test

- **Status:** todo
- **Depends on:** M9-03, M9-04, M9-05, M9-06
- **Docs:** [13 Soak test](../13-testing-and-benchmarks.md#soak-test)
- **Do:** Build the 72-hour soak harness with sleep and wake cycles, network changes, a steady log stream with the GUI open, repeated Compose up and down, pulls and prunes. Record memory of `marsad`, the GUI and the VM over time.
- **Deliver:** soak harness and a results report.
- **Accept when:**
  - The run completes with no crash, no stuck state and no unbounded memory growth in any process.
  - All disk space is reclaimed at the end.

### M9-11 Security review (gate G6)

- **Status:** todo
- **Depends on:** M9-01, M9-02, M9-15, M8-08
- **Docs:** [11 Security](../11-security.md)
- **Do:** Prepare a review package (threat model, architecture, test results), complete an internal review against every threat, and arrange an independent review before release. Finalise `SECURITY.md` and the vulnerability response process, including advisory monitoring.
- **Deliver:** review report, resolved or accepted findings recorded, finalised `SECURITY.md`.
- **Accept when:**
  - Every threat in the threat model is marked mitigated with a test, or accepted with a written reason.
  - No high-severity finding is open.

### M9-12 Privacy and crash reporting

- **Status:** todo
- **Depends on:** M3-08, Q-10
- **Docs:** [11 Privacy](../11-security.md#privacy)
- **Do:** Confirm no network call leaves the machine except registry traffic, update checks and the configurable network probe; add a test that fails on any other outbound connection from `marsad` or the GUI. Implement opt-in crash reporting only if Q-10 says so; otherwise document that none exists.
- **Deliver:** the outbound-connection test, a privacy note for users.
- **Accept when:**
  - The test enumerates and justifies every outbound destination.
  - Telemetry is off by default and absent unless Q-10 adds an opt-in.

### M9-13 Collect a local diagnostics archive

- **Status:** done
- **Depends on:** M3-08
- **Docs:** [03 Responsibilities](../03-host-daemon.md#responsibilities), [11 Threat model](../11-security.md#threat-model) (T11)
- **Do:** Implement authenticated `Diagnostics.Collect`. Collect bounded, redacted daemon and guest console tails, derive the engine log tail from the captured guest console, and record safe settings, versions, engine state history, and network status. Write a single private local archive for review.
- **Deliver:** daemon diagnostics module and redaction tests.
- **Accept when:**
  - An authenticated caller can explicitly create one local archive and receive its path even when the engine is stopped.
  - The archive contains the approved logs and diagnostic state without planted secrets, environment values, or raw settings.
  - Retrying the same request ID returns the existing archive path without creating another archive.

### M9-15 Fuzz the Docker create-request inspector

- **Status:** todo
- **Depends on:** M4-06, M9-02
- **Docs:** [11 Process hardening on the host](../11-security.md#process-hardening-on-the-host), [07 Correctness requirements](../07-file-sharing-and-storage.md#correctness-requirements)
- **Do:** Exercise the production Docker create-request inspector with a committed corpus of representative and malformed request bodies. Check that malformed or unsupported inputs cannot bypass the bind-mount policy.
- **Deliver:** An inspector target and corpus in `fuzz/`, wired into the scheduled workflow.
- **Accept when:**
  - The target runs for a fixed budget in CI without crashes.
  - Any crash found is fixed and its input added as a regression test.
