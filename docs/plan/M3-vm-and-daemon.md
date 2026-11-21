# M3 VM and daemon

**Outcome:** `marsad` creates, boots, supervises and stops the VM through the `vz` backend, exposes the typed API, and survives sleep, wake and crashes.

Design: [03 Host daemon](../03-host-daemon.md), [04 Virtualization on macOS](../04-virtualization-macos.md).

---

### M3-01 Hypervisor trait and fake backend

- **Status:** done
- **Depends on:** M0-04, M1-05
- **Docs:** [03 The Hypervisor trait](../03-host-daemon.md#the-hypervisor-trait), [13 The fake backend](../13-testing-and-benchmarks.md#the-fake-backend)
- **Do:** Implement `marsa-vm`: the `Hypervisor` and `Vm` traits, `VmSpec`, `Capabilities`, state and event types, and errors. Implement `FakeHypervisor` with scripted state transitions, a loopback "vsock" to a scripted agent, and failure injection.
- **Deliver:** `crates/marsa-vm` with the fake behind a feature or test module.
- **Accept when:**
  - The fake can simulate: normal boot, boot timeout, sudden exit, slow handshake, pause and resume.
  - The trait has no Apple-specific type in its signature.

### M3-02 `vz` backend: configure and boot

- **Status:** done
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

- **Status:** done
- **Depends on:** M3-02
- **Docs:** [04 Virtual hardware](../04-virtualization-macos.md#virtual-hardware)
- **Do:** Attach the read-only root disk and the read-write data disk with the mechanism chosen in M1-03, write the console to `console.log`, and implement `connect_vsock` returning an async stream.
- **Deliver:** backend code and Tier 2 tests.
- **Accept when:**
  - The agent handshake completes over vsock.
  - The console log contains the full boot output.
  - The root disk is not writable from the guest.

### M3-04 Qualify strict native IPv4 NAT

- **Status:** blocked (external HTTPS over native NAT times out or DHCP fails on the Tier 2 Mac)
- **Depends on:** M3-03, M3-16
- **Docs:** [04 Virtual hardware](../04-virtualization-macos.md#virtual-hardware), [06 Network modes](../06-networking.md#network-modes)
- **Do:** Qualify the native VZ attachment with the strict IPv4 guest test on Apple silicon.
- **Deliver:** The entitled native-only DHCP, gateway and HTTPS test and recorded passing hardware evidence.
- **Accept when:**
  - A guest in explicit native NAT mode obtains IPv4, the backend reports its address and gateway, and the guest reaches HTTPS with certificate verification.

The entitled tests on macOS 27.2.1 verified the read-only share at its
original host path, guest balloon inflation, and pause and resume events.
Marsa acquired a DHCP lease on one run and resolved the HTTPS destination.
The guest request timed out while the host could reach the destination
directly. Other runs received no lease. In a separate probe with a temporary
static address, the guest reached the NAT gateway and the backend reported
the same address and gateway as the guest kernel. A stock Ubuntu 26.04 VM
under Lima 2.3.0 obtained a DHCP lease and also timed out on HTTPS bound to
its Apple `vzNAT` interface.

The native qualification is independent of the VZ device and runtime work.
Run the strict test on Apple silicon with
`MARSA_ENTITLED_NATIVE_IPV4=1 bash crates/marsa-vm-vz/tests/run-entitled.sh target/guest`.

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

- **Status:** done
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

- **Status:** done
- **Depends on:** M3-03, M3-06, M2-06
- **Docs:** [05 marsa-agent](../05-guest-image.md#marsa-agent)
- **Do:** Implement the host side of the control protocol: connect, handshake with version check, request and response with timeouts, the event streams from the guest, and reconnect after pause and resume. Treat all guest input as untrusted.
- **Deliver:** the guest-control module in `marsad` with tests against a scripted agent.
- **Accept when:**
  - An incompatible guest protocol version produces a clear, user-visible error.
  - Oversized or malformed guest messages close the connection without crashing the daemon.
  - Port, health and stats events are delivered to subscribers.
  - An orderly host stop sends the guest shutdown request before using native fallback.

**Verification.** The scripted backend covered incompatible versions, malformed and oversized messages, partial frames, request IDs and result types, request timeouts, all three event types, and pause and resume with fresh time sync and a separate Docker ping. Shutdown tests covered a full command queue, a paused guest, and failure after the guest acknowledged shutdown. The production VZ backend, guest client, supervisor and typed Unix-socket API reached Docker Ready on Apple silicon macOS 27.2.1. The API served live guest health and stats. `Engine.Stop` left the persistent ext4 disk clean. Workspace tests, strict Clippy and dependency policy checks passed.

### M3-08 Typed API: Engine, Settings, Events, Stats

- **Status:** done
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

- **Status:** blocked (Q-04 and a clean second Apple silicon Mac pending)
- **Depends on:** M3-03, Q-04
- **Docs:** [12 Signing and notarization](../12-build-release-update.md#signing-and-notarization), R-09
- **Do:** Produce a Developer ID signed, hardened-runtime build of `marsad` with the virtualization entitlement, notarize it, and boot the VM on a second, clean Mac. This proves the signing and entitlement path long before release.
- **Deliver:** a first version of `xtask sign`, the entitlements file, a short report.
- **Accept when:**
  - The notarized build boots the VM on a Mac that has no developer tools installed.
  - The exact entitlements used are recorded in [12](../12-build-release-update.md).

Confirm the project-owned Developer ID and update-key custodian in Q-04. G3 also requires access to a second Apple silicon Mac running macOS 27 or later with developer tools absent. The native entitlement and boot path have been verified with an ad-hoc signed test on the first Mac.

### M3-11 Power and network watcher

- **Status:** todo
- **Depends on:** M3-07, M3-16
- **Docs:** [03 Sleep, wake and network changes](../03-host-daemon.md#sleep-wake-and-network-changes)
- **Do:** Subscribe to host sleep, wake and network-change notifications. Pause on sleep, resume on wake, push the current time to the agent, and publish events. Network-change handling here only emits the event; M4 consumes it.
- **Deliver:** the watcher in the macOS platform module, with the logic tested through an injectable event source.
- **Accept when:**
  - After a real sleep and wake, the guest clock is within one second of the host (Tier 2 or manual test recorded).
  - The sequence is covered by tests with injected events.

### M3-12 Compose the packaged VM runtime

- **Status:** todo
- **Depends on:** M3-03, M3-07, M3-08, M3-15, M3-16
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle), [04 Boot sequence](../04-virtualization-macos.md#boot-sequence), [12 App bundle layout](../12-build-release-update.md#app-bundle-layout)
- **Do:** Wire the packaged daemon to the native VZ backend, guest artefact validation, guest-control session and typed API. Create the private sparse data disk on first activation. Build each VM spec from the current settings and apply restart-scoped settings on the next activation. Publish health and stats from the active control session. Install the signed user-space helper and use the boot selector when network mode is automatic.
- **Deliver:** Runtime composition in `marsad` and an entitled system test through the normal daemon socket.
- **Accept when:**
  - A packaged daemon started with a fresh per-user state directory accepts a real `Engine.Start`, reaches Ready through the agent and Docker API, and reports guest health and stats.
  - Subsequent Stop and Restart use the same data disk. A restart-scoped settings change becomes effective on restart.
  - Invalid or missing guest files fail cleanly before a VM starts and preserve existing user data.

### M3-13 Keep guest control available without an upstream lease

- **Status:** done
- **Depends on:** M2-05, M2-07
- **Docs:** [05 Boot and init](../05-guest-image.md), [06 Network modes](../06-networking.md#network-modes)
- **Do:** Bring up guest loopback as a boot requirement. Keep the bounded initial upstream setup, but continue with the control agent and Docker if the upstream link or DHCP setup fails. Record the reason on the guest console. Keep the data disk and service failure handling.
- **Deliver:** Guest init change, regression tests for the setup sequence, and an entitled boot test with no upstream link or DHCP.
- **Accept when:**
  - Successful startup still uses DHCP, and an explicitly disabled interface still skips it.
  - A failed upstream link or DHCP attempt keeps the control agent and the local Docker API available. The guest logs why upstream setup failed.
  - A failure to configure loopback still stops startup.

### M3-14 Qualify the unprivileged user-space transport

- **Status:** done
- **Depends on:** M3-03, M3-13
- **Docs:** [06 Network modes](../06-networking.md#network-modes), [11 Security](../11-security.md)
- **Do:** Attach a private datagram transport to the VZ guest. Pin and supervise the user-space helper. Qualify a helper configuration that honors the scoped macOS resolver, restricts host service access and uses ordinary host egress policy. Leave boot mode selection to M3-15 and production port forwarding to M4-10.
- **Deliver:** A supported VZ user-space attachment, dependency and licence evidence, and an entitled system test.
- **Accept when:**
  - The packaged guest obtains a lease and reaches a configured HTTPS endpoint with valid TLS through a user-space-only attachment.
  - The helper runs unprivileged, has no public listeners, and releases all resources on stop or abnormal exit.
  - Tests verify scoped resolver behavior and denied access to unapproved host loopback and metadata endpoints.
  - Tests cover the helper being unavailable or blocked and show a bounded, diagnosable outcome.
- **Evidence:** The entitled VM obtained a DHCP lease, resolved a temporary macOS scoped DNS record and pulled an image from Docker Hub over verified TLS on macOS 27.2.1 with NordVPN, LuLu and the macOS firewall active. The socketpair and Rust tests verify destination restrictions, helper readiness, timeout, restart and cleanup.

### M3-15 Select the network mode before Docker starts

- **Status:** done
- **Depends on:** M3-06, M3-07, M3-08, M3-14
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle), [06 Automatic mode selection](../06-networking.md#automatic-mode-selection)
- **Do:** Add versioned guest control to synchronize time and probe HTTPS before allowing Docker to start. Pair guest and host evidence by destination, address family and policy. Select native NAT or one user-space candidate for an automatic boot. Keep configured modes pinned and release the engine even when external connectivity is unavailable. Leave packaged runtime wiring to M3-12 and port forwarding and network-change transitions to M4-10.
- **Deliver:** Guest control negotiation, typed connectivity results and a boot-only supervisor selector with fake and entitled system tests.
- **Accept when:**
  - A healthy native path and explicitly selected modes never restart to switch.
  - With native IPv4 forwarding unavailable and equivalent host HTTPS healthy, auto selects a verified user-space path before starting Docker.
  - A host failure, unknown proxy policy, TLS validation failure or incompatible guest leaves local Docker available without a false recovery claim.
  - The supervisor confirms the first guest is stopped before reusing its data disk, and exposes the active mode and failure stage in daemon status and events.
- **Evidence:** The entitled daemon API test on macOS 27.2.1 started Docker through its local socket after the selector reported a native IPv4 route failure and verified the user-space attachment over HTTPS. NordVPN, LuLu and the macOS firewall were active. The test received the selected attachment and failure stage from the daemon event stream.

### M3-16 Integrate VZ devices independently of native egress

- **Status:** done
- **Depends on:** M3-03, M3-14
- **Docs:** [04 Virtual hardware](../04-virtualization-macos.md#virtual-hardware), [06 Network modes](../06-networking.md#network-modes)
- **Do:** Integrate the existing native virtiofs, balloon, pause, resume and guest IPv4 facts support with both VZ network attachments. Keep the strict native NAT entitlement test independently selectable.
- **Deliver:** VZ device support, backend capabilities, and separate entitled device and native qualification tests.
- **Accept when:**
  - A shared folder mounts at its original path with the configured read-only access.
  - Balloon inflation, pause and resume work on Apple silicon and state changes are published.
  - The user-space attachment still passes HTTPS verification and the strict native test still requires IPv4, gateway and HTTPS through native NAT.
- **Evidence:** The entitled suite on macOS 27.2.1 passed read-only virtiofs, balloon inflation, pause and resume, guest IPv4 facts, and a TLS-verified registry pull through the user-space attachment. The separately selected native test recorded five DHCP discovers and no IPv4 lease.
