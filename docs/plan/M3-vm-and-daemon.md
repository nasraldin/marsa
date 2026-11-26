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

- **Status:** done
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

On 2026-11-25, the production VZ backend passed the strict native-only
test on a physical M1 Max running macOS 27.2.1 (26B131). A previously
packaged guest passed once. An image assembled from the current
`marsa-init` and `marsa-agent` at `fa621d8`, with the same pinned Linux
7.3.5 kernel and Alpine 3.24.4 base, passed on two consecutive cold boots
(6.76 s and 6.61 s). Each boot acquired an IPv4 DHCP lease, the backend's
address and default gateway matched the guest kernel, and the guest
reached the Docker registry over IPv4 with certificate verification.

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

- **Evidence:** Xcode 27.2 used the existing local Developer ID Application identity and signed-in account to submit and export the daemon qualification archive. The exported bundle passed strict signature and staple validation and retained only the virtualization entitlement on the daemon. With a private home, the exported daemon selected Userspace networking, reached Ready on the physical M1 Max with macOS 27.2.1, and answered Docker `/_ping`. An earlier guest fixture failed at the handshake. The passing test used the previously qualified native-init fixture.

G3 still requires the clean second physical Apple silicon boot. Apple's
`VZGenericPlatformConfiguration.isNestedVirtualizationSupported` returned
`false` on the M1 Max host. In a fresh Tart guest restored from Apple's macOS
27.2.1 image, the operator verified assigned IPv4, a responding default gateway,
and HTTPS by IP and DNS name. The ZIP checksum passed, Gatekeeper accepted the
app with a test quarantine marker, and first launch created the daemon socket.
The Linux VM boot inside that guest is outside the host's capabilities.
Project custody and update-key ownership remain tracked by Q-04. [Tart nested virtualization support](https://tart.run/faq/#nested-virtualization-support).

### M3-11 Power and network watcher

- **Status:** done
- **Depends on:** M3-07, M3-16
- **Docs:** [03 Sleep, wake and network changes](../03-host-daemon.md#sleep-wake-and-network-changes)
- **Do:** Subscribe to host sleep, wake and network-change notifications. Pause on sleep, resume on wake, push the current time to the agent, and publish events. Network-change handling here only emits the event; M4 consumes it.
- **Deliver:** the watcher in the macOS platform module, with the logic tested through an injectable event source.
- **Accept when:**
  - After a real sleep and wake, the guest clock is within one second of the host (Tier 2 or manual test recorded).
  - The sequence is covered by tests with injected events.
- **Evidence:** Injected sleep and wake, startup races, manual pause ownership, and daemon event ordering passed. On 2026-11-25, the entitled harness on an M1 Max running macOS 27.2.1 observed the owner put the Mac to sleep at 23:11:54 and wake it at 23:12:04 local time (UTC+04:00). The typed event stream delivered `WillSleep → Paused → Woke → Ready` in order. Docker reported a maximum bracketed guest clock skew of 2.867 ms before sleep and 2.009 ms after wake, within the one-second limit. The test used the packaged guest and user-space network helper.

### M3-12 Compose the packaged VM runtime

- **Status:** done
- **Depends on:** M3-03, M3-07, M3-08, M3-15, M3-16
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle), [04 Boot sequence](../04-virtualization-macos.md#boot-sequence), [12 App bundle layout](../12-build-release-update.md#app-bundle-layout)
- **Do:** Wire the packaged daemon to the native VZ backend, guest artefact validation, guest-control session and typed API. Create the private sparse data disk on first activation. Build each VM spec from the current settings and apply restart-scoped settings on the next activation. Publish health and stats from the active control session. Install the signed user-space helper and use the boot selector when network mode is automatic.
- **Deliver:** Runtime composition in `marsad` and an entitled system test through the normal daemon socket.
- **Accept when:**
  - A packaged daemon started with a fresh per-user state directory accepts a real `Engine.Start`, reaches Ready through the agent and Docker API, and reports guest health and stats.
  - Subsequent Stop and Restart use the same data disk. A restart-scoped settings change becomes effective on restart.
  - Invalid or missing guest files fail cleanly before a VM starts and preserve existing user data.
- **Evidence:** An ad-hoc-signed bundle on macOS 27.2.1 reached Docker Ready over its Unix socket on a fresh default 64 GiB sparse disk. It verified the user-space fallback with the native route unavailable and exposed health and stats. Restart applied new memory on the same disk inode. A missing image, corrupt image, and broken resource seal preserved the disk.

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

### M3-17 Reserve lifecycle admission for maintenance

- **Status:** done
- **Depends on:** M3-06, M4-02
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle)
- **Do:** Add an exclusive supervisor permit for daemon maintenance. Optionally compare the caller's preview with the lifecycle revision in the actor. Suspend automatic retry and idle stop, and block ordinary lifecycle and new Docker connections while the permit is held. Let the holder stop the guest or start one candidate.
- **Deliver:** A daemon-internal admission interface and fake backend tests for proxy activation and factory reset.
- **Accept when:**
  - Two callers cannot reserve maintenance at once. A stale preview is rejected before admission.
  - Ordinary start, stop, restart, pause, resume, and Docker connection requests return busy while maintenance is reserved.
  - The holder can stop or cancel the current boot, then start a candidate with no automatic retry. An unfinished candidate is cleaned up before the permit releases its reservation.
  - Cancellation of a waiting reservation and release of a held permit leave ordinary admission usable.

### M3-18 Automate Tart qualification for signed macOS builds

- **Status:** done
- **Depends on:** M3-12, M3-14
- **Docs:** [12 Signing and clean-Mac handoff](../12-build-release-update.md#signing-and-clean-mac-handoff)
- **Do:** Run the signed qualification build's network, Gatekeeper and first-launch checks from the host through Tart Guest Agent on a disposable macOS 27 or newer guest.
- **Deliver:** A host runner, a reusable guest script, a dedicated graphical viewer setup and exact qualification instructions.
- **Accept when:**
  - The test validates guest IPv4, gateway reachability and direct and DNS-based HTTPS without changing host controls.
  - The guest verifies the ZIP against the host's trusted digest, checks the signature and enabled Gatekeeper with a test quarantine marker, then launches with a clean Marsa home and observes the daemon socket becoming ready.
  - An agent-enabled macOS 27 guest completes the checks from the host without manual input inside the VM.
- **Evidence:** Tart 2.42.0 and Guest Agent 0.15.0 ran the checks on a fresh Apple
  macOS 27.2.1 restore using the verified user-space attachment with NordVPN and
  the host firewall enabled. IPv4, gateway reachability, both HTTPS probes, the
  archive digest, the code signature and Gatekeeper passed. A uniquely
  identified Tart viewer resolved the window targeting timeout while CI kept
  running. The agent selected Open in macOS's normal first-open dialog and the
  runner confirmed daemon readiness. A separate guest check verified the
  daemon socket existed with mode `0600` and Gatekeeper remained enabled.


### M3-19 Qualify default-capacity cold boot

- **Status:** done
- **Depends on:** M3-12
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle), [07 Data disk](../07-file-sharing-and-storage.md#data-disk)
- **Do:** Diagnose and eliminate timing failures when the packaged VM starts with a new default 64 GiB data disk, including after cancellation of a first start. Record preparation, agent connection and Docker readiness separately. Preserve the complete blank-device verification for disks of unknown provenance.
- **Deliver:** Any required production boot or disk-preparation changes and signed native qualification.
- **Accept when:**
  - Repeated fresh and interrupted-first-start boots of the default-capacity disk reach Ready within the daemon's documented startup budget and shut down cleanly.
  - Existing foreign and nonblank disks still fail closed without being formatted.
  - The qualification documents the tested pinned guest and uses the same settings in the fresh and interrupted cases. Include the factory memory policy. Check the complete guest console across automatic retries for kernel faults.

The pinned Linux 7.3.6 guest uses aligned direct I/O on the validated disk
descriptor for the full blank scan. A buffered scan faulted in kernel
readahead on the same kernel image. Three signed six-case runs with direct
I/O passed on Mac15,11 with the factory memory policy. In the third run,
all six reached Ready in 11.2–14.7 s with clean ext4 shutdown. The consoles
showed one boot for fresh starts and two for intentional interruptions.
The guest also refused a partitioned disk and a disk with data only in its
final sector, preserving both images byte for byte. The underlying kernel
defect remains unidentified.

On November 26, five fresh profiles using factory settings on a Mac15,11
(2 guest CPUs, 8 GiB guest RAM, 64 GiB sparse disk) each reached Docker
Ready and stopped. One console recorded a Linux 7.3.5 kernel fault during
disk preparation, followed by a second kernel boot before Ready. The other
four consoles had no kernel fault. The earlier 1 GiB results below qualify
that configuration. This observation prompted the factory-size qualification above. Automatic recovery can mask the
first failed boot in the final CLI exit status.

A separately built Linux 7.3.6 stable candidate, checked against the
upstream SHA-256, also faulted and exceeded the 75 s startup budget. A
Linux 6.18.65 long-term candidate using the same root filesystem and kernel
configuration fragment passed all six factory-size cases on this Mac.
Fresh, graceful cancellation and abrupt interruption reached Ready in
9.68, 2.19 and 9.08 s respectively in user-space mode and 10.59, 2.49 and
9.50 s in auto mode. Each abrupt interruption left an unallocated disk
and its original inode survived recovery. The graceful cancellation can
finish formatting before power-off, so the fixture exercises abrupt loss
during the blank scan separately. The candidate also passed the local G4
suite in 142 s. D-025 requires owner approval to adopt the long-term series.

The M6-10 default-capacity fixture twice stopped after the guest's system
mount, before formatting an entirely sparse 64 GiB new image. Prior
64 GiB boots succeeded. The subsequent 1 GiB fixture reached Docker 29.10.0
and shut down cleanly. The two results establish a timing risk and a
working packaged control path.

The signed qualification used Virtualization.framework on an Apple M1
Max with macOS 27.2.1, two guest CPUs, 1 GiB guest RAM, a 64 GiB data
disk and idle stop disabled. The package used the pinned Linux 7.3.5,
Alpine 3.24.4, and Docker 29.10.0 (commit `06db173`). Fresh and
interrupted starts used identical settings within each network mode.
The interrupted start requested Stop as soon as the guest reported
`mount_system` and restarted with the same wholly sparse inode.

The serial scan with only a faster byte comparison reproduced a
handshake failure. The host observed it at 62.303 s, with the disk probe
complete and the blank scan still running. The qualified implementation
uses four exact readers with 1 MiB of buffer each. Together they cover
the whole device, including its final sector. Only a completely successful
zero scan can reach formatting. The existing ext4 checks still precede
any filesystem repair.

Disk preparation measures the first guest's `data_disk` stage. Agent
connection is the observed time from Start to successful control
negotiation in the guest that subsequently reaches Docker Ready. Docker
Ready is the host Start response. Auto switched network backends during
acquisition. The existing 75 s daemon startup budget was retained.

| Network and start | Baseline disk (s) | Baseline Ready (s) | Revised disk (s) | Revised agent (s) | Revised Ready (s) | Revised Stop (s) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Userspace fresh | 30.153 | 34.949 | 23.203 | 25.930 | 27.129 | 0.228 |
| Userspace interrupted | 32.735 | 36.843 | 24.681 | 28.115 | 29.915 | 0.308 |
| Auto fresh | 40.965 | 61.812 | 25.621 | 42.485 | 52.114 | 0.334 |
| Auto interrupted | 28.601 | 48.306 | 21.491 | 40.837 | 46.336 | 0.321 |

A prior signed run of the same four-reader scan also passed all four
cases, with Ready at 20.429 s and 21.625 s in userspace and 47.713 s
and 47.273 s in Auto. A signed guest refused an existing DOS-partitioned
disk and a disk with an unrecognized marker in its last 4 KiB. Neither
reached formatting or agent startup. Both retained their marker,
capacity, inode, allocated blocks and modification timestamp.

The signed package exercised guest source `7550907f`. Its host daemon
and pinned kernel matched the baseline package. The ARM64 Linux guest
unit suites passed 43 tests and Clippy passed with warnings denied.
