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

### M9-03 Explicit disk space reclaim

- **Status:** done
- **Depends on:** M4-07, M2-08, M1-03, M5-05
- **Docs:** [07 Returning space to macOS](../07-file-sharing-and-storage.md#returning-space-to-macos), V-05, R-14
- **Do:** Implement authenticated `Disk.Reclaim` and wire `marsa disk reclaim`. Send the existing Trim command to the ready guest on explicit request. Measure actual host sparse allocation before and after, and report the observed delta separately from guest bytes submitted for discard. Keep automatic and post-cleanup reclaim in M9-14.
- **Deliver:** daemon and CLI integration, unit tests, and a Tier 2 disposable-data test.
- **Accept when:**
  - After deleting several GiB of disposable data in a dedicated test VM, explicit reclaim drops host image allocation by at least 90% of the freed amount.
  - A stopped VM does not boot for reclaim. Reclaim only runs on explicit request and concurrent or retried requests remain bounded.

The signed macOS 27.2.1 fixture freed 3,054,608,384 guest filesystem bytes
after deleting a disposable container layer. Reclaim released 3,221,209,088
host bytes from the same sparse image, and the guest passed the ext4
clean-shutdown check. The CLI and request UUID replay were verified against
the same running guest.

The updated signed test with lifecycle admission and the current guest
agent freed 2,974,916,608 guest bytes and released 3,221,213,184 host
bytes. It verified CLI output, UUID replay and clean ext4 shutdown.
The fake-runtime regression shows that a held maintenance permit
rejects reclaim before the guest receives a trim.

### M9-04 Memory behaviour

- **Status:** done
- **Depends on:** M4-02, M3-12
- **Docs:** [04 Memory management](../04-virtualization-macos.md#memory-management), V-04, R-03
- **Do:** Measure how much memory the balloon actually returns to macOS under `vz` (V-04) and record it. Implement the memory policy: right-sized default, balloon on idle if it helps, idle stop as the dependable mechanism, all configurable.
- **Deliver:** memory policy in the supervisor, a measurement report in `docs/spikes/M9-04-memory.md`.
- **Accept when:**
  - The report states measured host memory before, during and after a 2 GiB workload, with and without ballooning.
  - With idle stop on, host memory returns to the daemon's idle footprint after the timeout.
  - V-04 is resolved in the docs.

Run the offline workload in the signed packaged VZ runtime with user-space
networking. VZ memory devices and idle-stop transitions are shared across
attachments. Strict native IPv4 DHCP, gateway and external HTTPS validation
remain in M3-04 under [D-028](../15-decisions.md#d-028-qualify-native-ipv4-independently-of-runtime-and-power-management).

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

### M9-08 Clean uninstall

- **Status:** blocked (M4-04)
- **Depends on:** M4-04, M9-18
- **Docs:** [12 Uninstall](../12-build-release-update.md#uninstall), Q-08
- **Do:** Implement and document the uninstall procedure, including the matching removal of optional Docker integration installed by M4-04. Verify the procedure on a clean machine.
- **Deliver:** uninstall script or command and a clean-machine test.
- **Accept when:**
  - After uninstall, no Marsa file, launch agent, Docker context, symlink or PATH entry remains (verified by the test).

### M9-17 Confirm and reset daemon settings

- **Status:** done
- **Depends on:** M3-08
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api), [07 Integrity](../07-file-sharing-and-storage.md#integrity)
- **Do:** Add owner-authenticated settings reset to `Engine.Reset`. Prepare a short-lived single-use confirmation bound to the current settings revision and private configuration member. After confirmation, atomically persist defaults and publish the settings revision.
- **Deliver:** settings-only reset RPC and fixture tests.
- **Accept when:**
  - A valid confirmation resets settings without starting the engine or touching its disk or grants.
  - Missing, expired, repeated and stale confirmations cannot apply a reset. Unsafe or substituted configuration members remain intact.
  - Interrupted persistence leaves either the existing complete settings or the complete defaults.

**Verification.** Tests use the authenticated Unix API and private fixture
storage. They cover expiry, duplicate requests, revision changes and inode
substitution. Killing the staging fixture after file sync and before rename
preserved its original complete configuration. The following reset persisted
the complete defaults.

### M9-18 Recover private storage with confirmed factory reset

- **Status:** done
- **Depends on:** M9-17, M4-07, M8-01, M3-17
- **Docs:** [07 Integrity](../07-file-sharing-and-storage.md#integrity), [12 Uninstall](../12-build-release-update.md#uninstall)
- **Do:** Implement factory preparation and confirmation in `Engine.Reset`. Quiesce activation and permission writes, recreate the data disk, clear grants and reset settings. Recover any interrupted transaction on daemon restart.
- **Deliver:** factory reset, recovery logic and private storage fault-injection tests.
- **Accept when:**
  - Factory reset works even when the engine cannot start.
  - The confirmed reset recreates the data disk, clears grants and restores settings defaults. Unconfirmed or stale requests leave all three intact.
  - After interruption the daemon either restores the prior disk, grants and settings together or completes the confirmed reset before serving requests. Unrelated files remain intact.

**Implementation plan.** Quiesce the supervisor actor before taking custody
of storage. Serialize state readers and writers during the transaction.
Prepare replacements in a private journal on the same filesystem and record
the expected member identities before renaming any live member. Identity
checks include filesystem creation time to distinguish reused inodes. Keep originals
until a durable commit record exists. Recover before reading settings or
binding sockets and refuse any unrecognized member. Exercise every rename
and journal boundary with disposable storage. Recovery also removes abandoned
replacement files from the private staging directory before serving requests.

**Verification.** Disposable APFS fixtures recovered from 92 interruptions
during staging, replacement and cleanup, plus 36 interruptions during
rollback itself. Owner-authenticated fake and signed packaged-daemon tests
confirmed factory reset with a failed boot. The signed fixture did not start
a VM. A pre-publication interruption left the original settings, disk and
grants intact, and a subsequent recovery removed the abandoned stage. A
redirected stage member was refused and its target remained intact.

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
- **Do:** Enumerate and test outbound connections from `marsad` and the GUI. Add crash reporting with explicit opt-in and document the approved report fields, destination and retention under [D-037](../15-decisions.md#d-037-offer-opt-in-crash-reporting).
- **Deliver:** the outbound-connection test, a privacy note for users.
- **Accept when:**
  - The test enumerates and justifies every outbound destination.
  - Crash reporting is off by default. Outbound tests cover disabled, enabled, and revoked consent.

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

### M9-14 Coordinate automatic reclaim with confirmed cleanup

- **Status:** todo
- **Depends on:** M9-03, M5-12, M7-11
- **Docs:** [07 Returning space to macOS](../07-file-sharing-and-storage.md#returning-space-to-macos), [07 Disk cleanup with a preview](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
- **Do:** Reclaim after applying a confirmed cleanup plan and on a schedule when the VM is idle. Honor the setting and verify that automatic admission remains safe under concurrent disk activity. Wire the GUI cleanup flow and qualify any required offline compaction fallback for a backend without discard.
- **Deliver:** automatic admission and cleanup integration, GUI reporting, and a Tier 2 image cleanup test.
- **Accept when:**
  - A test that pulls several gigabytes of images, removes them and reclaims shows the image file's real size on the host dropping by at least 90% of the freed amount.
  - Reclaim never runs during heavy I/O without the user asking.

### M9-15 Fuzz the Docker create-request inspector

- **Status:** done
- **Depends on:** M4-06, M9-02
- **Docs:** [11 Process hardening on the host](../11-security.md#process-hardening-on-the-host), [07 Correctness requirements](../07-file-sharing-and-storage.md#correctness-requirements)
- **Do:** Exercise the production Docker create-request inspector with a committed corpus of representative and malformed request bodies. Check that malformed or unsupported inputs cannot bypass the bind-mount policy.
- **Deliver:** An inspector target and corpus in `fuzz/`, wired into the scheduled workflow.
- **Accept when:**
  - The target runs for a fixed budget in CI without crashes.
  - Any crash found is fixed and its input added as a regression test.

The [hosted fuzz run](https://github.com/nasraldin/marsa/actions/runs/44974259069) completed the configured budget for all four targets without a crash.

### M9-19 Synchronize the crash-recovery test with daemon readiness

- **Status:** done
- **Depends on:** M9-06
- **Docs:** [13 System tests](../13-testing-and-benchmarks.md#system-tests-real-vm)
- **Do:** Synchronize crash injection with an authenticated daemon RPC. A transport handshake can complete while PID-file publication is still in progress.
- **Deliver:** corrected daemon lifecycle regression.
- **Accept when:**
  - The test proves the first process published its PID before injecting SIGKILL and verifies recovery by the replacement process.
  - Repeated execution and the daemon lifecycle test suite pass on the standard user account.

**Evidence.** The complete lifecycle test passed and the corrected crash case passed 24 consecutive runs on Apple silicon.
