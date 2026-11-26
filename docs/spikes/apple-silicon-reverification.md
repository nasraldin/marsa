# Apple silicon acceptance checks, November 26, 2026

This run used a Mac15,11 with macOS 27.2.1, Xcode 27.2 and Rust 1.100.0. The local Docker builder ran arm64 Linux under Lima. Guest and daemon tests used disposable disks and private state. The figures below describe this run on a development machine. The [milestone gates](../plan/README.md#gates) have separate release requirements.

## Foundation

| Task | Evidence |
|---|---|
| M0-01 | The locked workspace built on arm64. Formatting and workspace Clippy with warnings denied passed. All 29 direct runtime edges among the product crates matched the corrected architecture diagram, including target-specific dependencies. |
| M0-02 | The issue templates request a task ID. The PR template requests the task ID, acceptance tests, Verify findings and follow-ups. D-022 records the owner's MIT decision. Q-09 remains open. |
| M0-03 | Ran the xtask help, check and test entry points. A disposable workspace exposed a Cargo manifest lint that passed Clippy's compiler-level warning setting. The workspace now denies the default Cargo lint group. A dependency left unused in the disposable crate produces an error. |
| M0-04 | The settings round-trip, invalid-input and migration tests passed. External Docker discovery now derives the account owner's socket through the same path API as the daemon, including when the process home differs. |
| M0-05 | Built marsa-proto for the host and aarch64-unknown-linux-musl. Handshake, incompatible-major and oversized-frame rejection tests passed. The wire protocol documents global and message-specific bounds. |
| M0-06 | Checked Tier 1 on main and on the audit PR. Tier 2 remains a protected private-repository dispatch. Local hardware results below do not attest to a private Actions run. |
| M0-07 | A disposable checkout added a GPL-3.0-only path dependency. The actual xtask check rejected its licence. Changing the licence to MIT made cargo-deny pass. The daily RustSec job uses a failure exit for new warnings. |
| M0-08 | The checker verified links and task references and rendered 36 diagrams across 47 documents before this report was added. Its four tests included deliberately broken links, anchors, Mermaid and task references. |
| M0-09 | All six classifier tests passed. The documentation-only revision of the PR ran documentation validation and skipped the three build jobs. Later mixed revisions scheduled those jobs. |
| M0-10 | Checked decisions D-035 through D-037 against their implementation tasks. The plan index now also lists the outstanding name, signing, LAN-exposure and governance decisions. |

## Risk spikes

| Task | Observation |
|---|---|
| M1-01 | Reproduced all four original captures with the three IBM Plex Sans Arabic weights installed. The original widget still failed visual caret positioning. Logical deletion and mixed-direction rendering were exercised. |
| M1-02 | Both the Rust and Swift bridges booted the same stock guest, completed the 16-byte vsock challenge and observed shutdown. |
| M1-03 | After deletion, guest trim released 4,294,971,392 bytes in automatic mode, 4,294,995,968 in cached mode and 4,294,971,392 in uncached mode. |
| M1-04 | Host edits reached shared-file contents with zero host-origin inotify events in the five-second window. Guest edits produced two directory and two file events. In three 16,384-entry walks, native APFS took 139–231 ms and guest virtiofs took 1,391–3,859 ms. The runs shared the host with other verification work. |
| M1-05 | D-023 and D-024 record the owner's GUI and Apple bridge decisions. The original spike's caret result remains relevant to product qualification. |
| M1-06 | Exercised native editor, controls, locale/chart, lifecycle and stream-load probes. The lifecycle probe timed out once under load and passed on replay. The stream probe processed 200,000 log lines and rendered a bounded range of a 2,000-row container list. Spoken VoiceOver, physical Arabic arrows and live IME behavior in the production editor belong to M6-08. |

## Built guest and runtime

- **M2-01 through M2-04 and M2-09.** The initial build stopped at a missing Alpine APK. Updated the zstd package pin and source inventory together, verified the source and patch against the pinned aports recipe, and installed the signed APK offline. The guest build then produced byte-identical payloads across two builds, including the kernel, EROFS, manifest, SBOM and notice archive. The root filesystem tar measured 179,968,000 bytes and the EROFS image measured 88,862,720 bytes.
- **M2-05 through M2-08 and M2-10.** The real VM harness booted the production guest, negotiated with the agent, ran the pinned hello-world image and checked orderly shutdown and the clean ext4 state. The kernel took 73 ms, init to agent handshake took 521 ms and Docker to Ready took 707 ms. Fresh data-disk setup accounted for 406 ms. The handshake missed the proposed 300 ms budget. Fine-grained event latency and private runner execution require separate evidence.
- **M3.** Five production VZ system tests passed. A separate strict native NAT test verified IPv4 and registry HTTPS. Automatic selection initially reported host DNS unavailable and passed on retry with native NAT. After earlier attempts timed out waiting for system sleep, the operator put the physical Mac to sleep and woke it. The entitled M3-11 harness observed WillSleep, Paused, Woke and Ready in order. Maximum bracketed clock skew was 3.467 ms before sleep and 89.563 ms after wake, within the one-second limit. The signed login fixture registered a disposable LaunchAgent, observed restart after SIGKILL and verified unregistration. A real logout/login was not part of that test.
- **M4.** All 16 packaged-runtime tests passed together in 550 seconds. The rebuilt guest exercised Docker streams, scoped bind approval, on-demand start, Compose Watch, scoped DNS updates in running containers, BuildKit SSH sessions and manual proxy activation. The test-only SSH build image needed two updated Alpine packages. A missing proxy fixture previously let a selected traffic test return success without checking traffic. Tier 2 now stages the fixture, and a selected test without it fails explicitly.
- **M5 and M8.** The workspace suite exercised CLI lifecycle, JSON output, scoped MCP reads, grant revocation, owner approval and audit. Exact-identity cleanup and the agent mutation providers retain the blockers in their task files.
- **M6 and M7.** Five native snapshots initially picked up the system pointer over resting controls. Explicit pointer initialization made all 100 component snapshots pass against the existing references. These snapshots cover 25 component fixtures. The four-variant product-screen audit and owner's Arabic review are separate checks.
- **M9.** Both packaged disk compatibility tests passed, including preservation of image, container and volume state across an upgrade from the previously installed guest and refusal to write a newer disk schema. A disposable Tart guest passed the packaged uninstall test. That guest reported macOS 26.6.2, below the supported minimum. The full soak and independent security review remain open.
- **M10.** Staged verified release resources and assembled an unsigned 347,210,746-byte development bundle. The bundler explicitly reported that combined release notices and SBOM were absent. The result cannot qualify notarization, update or rollback.
- **M11-02.** Built libkrun 1.19.8 at the recorded upstream commit and booted the stock Marsa image to a validated guest greeting. The existing share-isolation blocker still applies. The load and filesystem figures above do not satisfy the comparative benchmark gate.

## Qualification still required

G3 requires a second physical Apple silicon Mac with no developer tools or previous Marsa installation, together with the Q-04 custody decision. This development Mac has Xcode and an installed Marsa app. G4 still requires the full Docker, Compose and Testcontainers conformance suite. G5 requires production accessibility, four-variant review and Arabic sign-off. G6 includes independent security review. G7 includes clean installation, update, rollback and the owner's release decision. The [task files](../plan/README.md) identify the remaining implementation dependencies.

## Packaged VM acceptance replay

| Task | Result on this Mac |
|---|---|
| M3-12 | The signed packaged daemon handled Engine.Start through its private socket, reported agent health and memory, and answered Docker inventory requests. A fresh 64 GiB disk reached Ready in 12.131 s. Restart applied the memory increase and preserved the inode. Missing rootfs, checksum mismatch and invalid bundle seal each failed at preparation, with the console and disk contents preserved. |
| M3-13 | All three Linux network-init tests ran as arm64 musl executables in Lima. They exercised required loopback setup, optional upstream link and DHCP failure, and the enabled and disabled DHCP paths. The entitled VZ suite also booted the packaged guest without upstream connectivity and reached local Docker. |
| M3-14 | The helper socketpair fixture passed DHCP, system DNS, loopback isolation and daemon lifetime. The Go tests covered metadata restrictions. A real VZ guest reached a verified registry over the user-space attachment. |
| M3-15 | All eight supervisor selector tests passed, including failed-stop disk exclusion, healthy and pinned modes and unverified alternate rollback. The physical host qualified healthy native NAT. Native route failure with an equivalent healthy host was exercised by the supervisor fixture. |
| M3-16 | The entitled device test exercised read-only sharing, balloon adjustment and pause/resume. Separate physical tests verified registry TLS over the user-space attachment and guest IPv4, gateway and HTTPS over strict native NAT. |
| M3-17 | All ten supervisor maintenance tests passed. They covered exclusive admission, stale revision, ordinary lifecycle and Docker gates, cancellation, candidate cleanup, suppression of retries and restoration of ordinary admission. |
| M3-18 | The Guest Agent responded on disposable clones of each of the three local Tart baselines. All reported macOS 26.6.2 with Gatekeeper enabled. The public release offers a beta DMG and checksums. The M3-10 signed qualification ZIP was absent from the searched project, download and desktop locations. This run did not repeat the macOS 27+ first-launch qualification. |
| M3-19 | A signed packaged regression fixture ran identical 2-CPU, 1 GiB RAM, 64 GiB disk settings for fresh and interrupted first starts. Userspace reached Ready in 10.809 s and 10.592 s respectively. Auto reached Ready in 12.351 s and 14.163 s. Cancellation followed the guest mount stage, retained the sparse inode, and each completed run shut down. The Linux disk tests also rejected ambiguous filesystems and nonzero data beyond the first MiB and at the end of a partial scan chunk. Guest manifest source was 9dfb4c63, Linux 7.3.5, Alpine 3.24.4 and Docker 29.10.0. |

## Docker integration acceptance replay

| Task | Result on this Mac |
|---|---|
| M4-01 | The packaged proxy served Docker version, build, interactive run and exec, log follow and attach. A 2,215,736,832-byte save increased measured daemon RSS by 16 KiB. After 140 canceled follow requests the upstream bridge still served clients. |
| M4-02 | Ten concurrent Docker CLI calls joined one cold activation. Running, paused and restarting containers deferred idle stop. The VM stopped once the workload stopped, then a new Docker request started it again. |
| M4-03 | The staged binaries reported Docker 29.10.0, Compose 5.8.0, Buildx 0.39.1 and credential helper 0.9.13. The workspace checksum-mismatch test verified failure without publishing a bundle. |
| M4-04 | The production Settings client created the system link under native macOS authorization, checked its destination and root ownership, and removed it. The test started and finished with no system link. The private fixture socket retained its inode. |
| M4-05 | Nine share-policy tests passed, including path traversal, aliases, protected ancestors and descendants, APFS alternate paths and the absence of a settings override. |
| M4-06 | Signed packaged bind-approval tests passed alongside the interactive Docker tests. They covered ownership, denial, session expiry and persistent approval. |
| M4-07 | The packaged daemon initialized the private sparse disk, reported guest and host usage, and toggled Time Machine exclusion. In a separate run a named volume survived SIGKILL and the recovered guest filesystem grew from 1,020,702,720 to 2,077,073,408 bytes. |
| M4-08 | The original guest failed the host-alias regression. After the fix a fresh native VM pulled BusyBox, answered TCP DNS and served host HTTP through all three aliases on both the default and a custom Docker bridge. A second physical Wi-Fi check on the fixed resolver preserved the container ID, public DNS, alias resolution and the host HTTP response. |
| M4-19 | The Compose Watch fixture served updated content and delivered a Hugo LiveReload frame for an ordinary save and an atomic replacement. The original process survived and ignored and out-of-scope files remained absent. |
