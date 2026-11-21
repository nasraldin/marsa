# M3 VM and daemon

**Outcome:** `marsad` creates, boots, supervises and stops the VM through the `vz` backend, exposes the typed API, and survives sleep, wake and crashes.

Design: [03 Host daemon](../03-host-daemon.md), [04 Virtualization on macOS](../04-virtualization-macos.md).

---

### M3-01 Hypervisor trait and fake backend

- **Status:** todo
- **Depends on:** M0-04, M1-05
- **Docs:** [03 The Hypervisor trait](../03-host-daemon.md#the-hypervisor-trait), [13 The fake backend](../13-testing-and-benchmarks.md#the-fake-backend)
- **Do:** Implement `marsa-vm`: the `Hypervisor` and `Vm` traits, `VmSpec`, `Capabilities`, state and event types, and errors. Implement `FakeHypervisor` with scripted state transitions, a loopback "vsock" to a scripted agent, and failure injection.
- **Deliver:** `crates/marsa-vm` with the fake behind a feature or test module.
- **Accept when:**
  - The fake can simulate: normal boot, boot timeout, sudden exit, slow handshake, pause and resume.
  - The trait has no Apple-specific type in its signature.

### M3-02 `vz` backend: configure and boot

- **Status:** todo
- **Depends on:** M3-01, M2-02, Q-02
- **Docs:** [04 Virtual hardware](../04-virtualization-macos.md#virtual-hardware), [04 Threading](../04-virtualization-macos.md#threading)
- **Do:** Implement `marsa-vm-vz` using generated `objc2-virtualization` bindings ([D-024](../15-decisions.md#d-024-use-direct-rust-bindings-for-virtualizationframework)). Build the VM configuration from `VmSpec`, validate it, start it and report state changes as a stream. Own a single serial queue for all framework calls.
- **Deliver:** `crates/marsa-vm-vz`.
- **Accept when:**
  - The Marsa kernel and image boot to `marsa-init`.
  - State events arrive for start, stop and unexpected exit.
  - Invalid specs fail with typed errors before start.
  - No framework object is touched off its queue (reviewed and documented).

### M3-03 `vz` backend: disks, console and vsock

- **Status:** todo
- **Depends on:** M3-02
- **Docs:** [04 Virtual hardware](../04-virtualization-macos.md#virtual-hardware)
- **Do:** Attach the read-only root disk and the read-write data disk with the mechanism chosen in M1-03, write the console to `console.log`, and implement `connect_vsock` returning an async stream.
- **Deliver:** backend code and Tier 2 tests.
- **Accept when:**
  - The agent handshake completes over vsock.
  - The console log contains the full boot output.
  - The root disk is not writable from the guest.

### M3-04 `vz` backend: shares, network, memory target, pause

- **Status:** todo
- **Depends on:** M3-03
- **Docs:** [04 Virtual hardware](../04-virtualization-macos.md#virtual-hardware), [06 Network modes](../06-networking.md#network-modes)
- **Do:** Attach virtiofs shares from `VmSpec`, the NAT network attachment, the balloon device with `set_memory_target`, and pause and resume. Report accurate `Capabilities`, including whether shares can be added without a restart.
- **Deliver:** backend code and Tier 2 tests.
- **Accept when:**
  - A shared folder is visible in the guest at its host path.
  - The guest has outbound connectivity in NAT mode and `network_info` reports its address.
  - Pause and resume work and are reported as events.

### M3-05 `marsad` skeleton

- **Status:** done
- **Depends on:** M0-04, M0-05
- **Docs:** [03 Process supervision](../03-host-daemon.md#process-supervision), [03 Observability](../03-host-daemon.md#observability)
- **Do:** Create the daemon binary: single-instance lock, directory creation with correct modes, structured logging with rotation, settings load, graceful shutdown on signals, and the gRPC server on `marsad.sock` with the `Meta` service only.
- **Deliver:** `crates/marsad`.
- **Accept when:**
  - A second instance exits with a clear message.
  - `~/.marsa/run` is mode 0700 and sockets are 0600 (tested).
  - `Meta.Version` answers over the socket.
  - SIGTERM shuts down cleanly and removes the socket and pid file.

### M3-06 VM supervisor state machine

- **Status:** todo
- **Depends on:** M3-01, M3-05
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle)
- **Do:** Implement the supervisor as an explicit state machine with every state and transition in the diagram: on-demand start with waiting callers, readiness requiring agent handshake and engine health, idle detection, failure capture with console tail, retry with backoff, and clean stop with a forced fallback.
- **Deliver:** the supervisor module with exhaustive tests against `FakeHypervisor`.
- **Accept when:**
  - Every transition in the diagram is covered by a test.
  - Concurrent `ensure_ready` callers all resolve from one boot.
  - A boot timeout yields `Failed` with the boot stage and console tail.
  - Retries stop after the configured limit.

### M3-07 Guest control client

- **Status:** todo
- **Depends on:** M3-06, M2-06
- **Docs:** [05 marsa-agent](../05-guest-image.md#marsa-agent)
- **Do:** Implement the host side of the control protocol: connect, handshake with version check, request and response with timeouts, the event streams from the guest, and reconnect after pause and resume. Treat all guest input as untrusted.
- **Deliver:** the guest-control module in `marsad` with tests against a scripted agent.
- **Accept when:**
  - An incompatible guest protocol version produces a clear, user-visible error.
  - Oversized or malformed guest messages close the connection without crashing the daemon.
  - Port, health and stats events are delivered to subscribers.

### M3-08 Typed API: Engine, Settings, Events, Stats

- **Status:** todo
- **Depends on:** M3-06
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api)
- **Do:** Implement the `Engine`, `Settings`, `Events` and `Stats` services. Build the event bus with a snapshot-then-stream contract. Settings updates validate, persist atomically and apply live where possible, reporting when a restart is required.
- **Deliver:** service implementations and integration tests using the fake backend.
- **Accept when:**
  - A client receives a snapshot followed by ordered events with no gap.
  - Invalid settings are rejected without changing the stored file.
  - `Engine.Start`, `Stop` and `Restart` drive the supervisor and are idempotent.

### M3-09 Start at login and daemon lifecycle on macOS

- **Status:** done
- **Depends on:** M3-05
- **Docs:** [03 Process supervision](../03-host-daemon.md#process-supervision), V-14
- **Do:** Verify the current API for registering a per-user agent and login item on the minimum macOS (V-14). Implement registration, a setting to turn start-at-login on and off, and automatic restart after a crash.
- **Deliver:** the macOS platform module code and the agent definition.
- **Accept when:**
  - `marsad` starts at login when enabled and does not when disabled.
  - Killing `marsad` results in an automatic restart.
  - V-14 is resolved in the docs.

### M3-10 Signed "hello VM" build (gate G3)

- **Status:** todo
- **Depends on:** M3-03, Q-04
- **Docs:** [12 Signing and notarization](../12-build-release-update.md#signing-and-notarization), R-09
- **Do:** Produce a Developer ID signed, hardened-runtime build of `marsad` with the virtualization entitlement, notarize it, and boot the VM on a second, clean Mac. This proves the signing and entitlement path long before release.
- **Deliver:** a first version of `xtask sign`, the entitlements file, a short report.
- **Accept when:**
  - The notarized build boots the VM on a Mac that has no developer tools installed.
  - The exact entitlements used are recorded in [12](../12-build-release-update.md).

### M3-11 Power and network watcher

- **Status:** todo
- **Depends on:** M3-07, M3-04
- **Docs:** [03 Sleep, wake and network changes](../03-host-daemon.md#sleep-wake-and-network-changes)
- **Do:** Subscribe to host sleep, wake and network-change notifications. Pause on sleep, resume on wake, push the current time to the agent, and publish events. Network-change handling here only emits the event; M4 consumes it.
- **Deliver:** the watcher in the macOS platform module, with the logic tested through an injectable event source.
- **Accept when:**
  - After a real sleep and wake, the guest clock is within one second of the host (Tier 2 or manual test recorded).
  - The sequence is covered by tests with injected events.
