# 15 Decision records

Each record states what was decided, why, what was rejected and what would reopen it. Records are numbered and never rewritten; a changed decision gets a new record that supersedes the old one.

To add a record: copy the template at the bottom, take the next number, and link it from the doc it affects.

## Index

| # | Decision | Status |
|---|---|---|
| D-001 | Reuse the Docker engine unmodified | Accepted |
| D-002 | Rust for all host code | Accepted |
| D-003 | Headless daemon; GUI is an optional client | Accepted |
| D-034 | Cache Docker inventory in the daemon | Accepted |
| D-004 | GPUI for the GUI, gated by a right-to-left spike | Superseded by D-023 |
| D-005 | Tauri 2 as the GUI fallback | Superseded by D-023 |
| D-006 | macOS on Apple silicon first | Accepted |
| D-007 | No Rosetta, no x86 images, no Intel Macs | Accepted |
| D-008 | Virtualization.framework default; libkrun benchmarked | Accepted |
| D-009 | One shared VM, not one VM per container | Accepted |
| D-010 | Alpine-based minimal guest with read-only root | Accepted |
| D-011 | Kernel NAT by default with automatic user-space fallback | Accepted |
| D-012 | Shared folders by explicit allowlist | Accepted |
| D-013 | MCP server in version 1 with permission tiers | Accepted |
| D-014 | Kubernetes after version 1 | Accepted |
| D-015 | English (US) and Arabic (MSA), LTR and RTL, light and dark | Accepted |
| D-016 | Product name: Marsa | Accepted |
| D-017 | Open source | Accepted; licence pending |
| D-018 | Windows: WSL2 default, HCS later if measured | Accepted |
| D-019 | Linux: native engine, no VM | Accepted |
| D-020 | Rootful engine inside the VM on macOS | Accepted |
| D-021 | Lima and Apple's container tool are not shipped | Accepted |
| D-022 | Licence Marsa under MIT | Accepted |
| D-023 | Choose GPUI for the desktop | Accepted |
| D-024 | Use direct Rust bindings for Virtualization.framework | Accepted |
| D-025 | Pin the current stable Linux kernel | Accepted |
| D-026 | Use EROFS for the guest root filesystem | Accepted |
| D-027 | Apply proxy settings at a controlled engine restart | Accepted |
| D-028 | Qualify native IPv4 independently of runtime and power management | Accepted |
| D-029 | Verify updates with TUF | Accepted |
| D-031 | Scope development file sync with Compose Watch | Accepted |
| D-033 | Keep the command-line interface in English | Accepted |
| D-035 | The project owner reviews Arabic terminology | Accepted |
| D-036 | Offer the optional Docker socket link in Settings | Accepted |
| D-037 | Offer opt-in crash reporting | Accepted |
| D-038 | Bind published ports to loopback by default | Accepted |

---

## D-001 Reuse the Docker engine unmodified

- **Decision:** Run upstream dockerd, containerd, runc, BuildKit and Compose as released. Replace only the GUI and the VM, file-sharing and networking layers.
- **Why:** The slowness users feel in Docker Desktop comes from the Electron GUI and the VM stack, not the engine. Reuse gives full Docker API compatibility and upstream fixes for free.
- **Rejected:** Rewriting the engine; forking Moby.
- **Reopen if:** Upstream licensing or distribution of the engine changes.

## D-002 Rust for all host code

- **Decision:** The daemon, CLI, MCP server, GUI and guest helpers are Rust.
- **Why:** Small footprint, no garbage-collection pauses, direct access to OS APIs, one language across host and guest. The compiler catches a large share of mistakes in AI-written code before anything runs.
- **Rejected:** Go (GC and a second language beside GPUI); Swift for the whole host (not cross-platform).

## D-003 Headless daemon; GUI is an optional client

- **Decision:** `marsad` owns all state and runs without a window. The GUI, CLI and MCP server are clients of one typed API.
- **Why:** Most of Docker Desktop's "heavy" reputation comes from the GUI being the product. A closed window should cost nothing.
- **Consequence:** The tray item belongs to the daemon. No logic lives only in the GUI.

## D-004 GPUI for the GUI, gated by a right-to-left spike

- **Decision:** Build the GUI with GPUI and gpui-component.
- **Why:** On performance and long-running stability, GPUI was the best of the options compared: no browser process, no JavaScript heap, one renderer with identical output on every OS. gpui-component supplies 60+ components, theming with light and dark, and virtualised tables and lists.
- **Known costs:** New accessibility support, an API that still changes, a smaller ecosystem, and weak right-to-left support.
- **Condition:** The spike in task M1-01 must pass the criteria in [08](08-gui.md#spike-pass-criteria-task-m1-01). If it fails, D-005 applies.
- **Rejected:** Electron (heaviest; the thing being replaced). Three native UIs (three codebases that drift; not automatically faster on Windows; fails "same experience").

## D-005 Tauri 2 as the GUI fallback

- **Decision:** If D-004's condition fails, build the GUI with Tauri 2 and a web front end.
- **Why:** Web rendering has complete right-to-left and accessibility support. Tauri is far lighter than Electron.
- **Known costs:** Webview memory growth on high-rate event streams was reported; three different rendering engines across platforms; weakest on Linux.
- **Mitigations if used:** Destroy the webview on window close; batch streams over a channel; capped virtualised log buffer.

## D-006 macOS on Apple silicon first

- **Decision:** Version 1 targets macOS on Apple silicon only.
- **Why:** The full three-platform scope is years of work for a small team. Docker Desktop's pain is worst on the Mac. Comparable products shipped one platform first.

## D-007 No Rosetta, no x86 images, no Intel Macs

- **Decision:** Only linux/arm64 images run. No Rosetta, no QEMU emulation, no Intel Mac support.
- **Why:** Modern platforms only. Apple is phasing Rosetta out for Mac apps after macOS 27, and the future of Rosetta for Linux VMs was not clearly stated at planning time.
- **Consequence:** amd64-only images do not run. This is an explicit, documented error. It also removed the main reason to be tied to Virtualization.framework.
- **Reopen if:** Users show strong need. The only route back is opt-in QEMU emulation, which is several times slower.

## D-008 Virtualization.framework default; libkrun benchmarked

- **Decision:** Ship `vz` as the default backend. Build `krun` behind the same trait and switch the default only if it passes the benchmark gate in [04](04-virtualization-macos.md#choosing-the-default-the-benchmark-gate).
- **Why:** `vz` is official and low-risk. The fastest Mac products use custom VMMs on Hypervisor.framework for memory return and file-sharing speed, so that is the performance ceiling; but stock libkrun was reported slower on one workload, so it must be measured.

## D-009 One shared VM, not one VM per container

- **Decision:** All containers run in one Linux VM with a standard Docker engine.
- **Why:** Docker API compatibility. Apple's per-container-VM model is good isolation but is not the Docker API.

## D-010 Alpine-based minimal guest with read-only root

- **Decision:** An Alpine-derived root filesystem, read-only and versioned with the app; a separate data disk; a Rust init instead of a general init system.
- **Why:** A few megabytes instead of a hundred or more, faster boot, nothing extra running. musl is not a problem because containers bring their own userland and the engine ships as static binaries.
- **Rejected:** Debian or Ubuntu (boot a general-purpose system); Fedora CoreOS and Flatcar (large, systemd-centred); a fully custom Buildroot image (a later optimisation).
- **Implementation (M2-03):** Use the pinned Alpine minirootfs. Alpine 3.24's `alpine-base` meta-package depends on OpenRC and installs service packages outside the guest contents table. The recipe adds only the locked tools and dependencies required by Marsa.

## D-011 Kernel NAT by default with automatic user-space fallback

- **Decision:** Use the OS's kernel-level NAT as the fast path. Switch automatically to a user-space stack when a VPN or firewall blocks it. Published ports go through a Rust forwarder in the daemon.
- **Why:** Kernel NAT is fastest; user-space stacks are slower but survive hostile networks.

## D-012 Shared folders by explicit allowlist

- **Decision:** The VM sees only folders the user chose. Credential directories can never be shared.
- **Why:** Sharing the home folder lets any container read SSH keys and cloud credentials.
- **Consequence:** A bind mount outside the allowlist is refused with instructions, or prompts the user.

## D-013 MCP server in version 1 with permission tiers

- **Decision:** Ship an MCP server in version 1. Read actions are free; operate actions ask once per client and project; destroy and host-affecting actions always ask.
- **Why:** Agent access is a differentiator and is cheap to build on the typed API, but the Docker socket equals root over the VM, so permissions come first.

## D-014 Kubernetes after version 1

- **Decision:** No Kubernetes in version 1.
- **Why:** A large feature many Docker users never open.

## D-015 English (US) and Arabic (MSA), LTR and RTL, light and dark

- **Decision:** Both languages are first-class with fully mirrored layouts. Light and dark themes follow the system by default. Every screen is built and tested in four variants.
- **Consequence:** Direction-aware layout is part of the component layer from the first commit. This requirement is what makes D-004 conditional.

## D-016 Product name: Marsa

- **Decision:** The product is Marsa (مرسى, "harbour"). CLI `marsa`, daemon `marsad`.
- **Open:** Trademark search in the software class, domain, package names. See [16](16-risks-and-open-questions.md).

## D-017 Open source

- **Decision:** Marsa is open source.
- **Open:** The licence. Apache-2.0 is recommended. No `LICENSE` file is added until the owner confirms.

## D-018 Windows: WSL2 default, HCS later if measured

- **Decision:** On Windows, use WSL2 with Marsa's own minimal distribution. Add a direct HCS backend only if Marsa's own measurements show WSL2 missing the memory or start-up targets.
- **Why:** WSL2 is low effort and universal. The HCS case rests on one self-reported benchmark.

## D-019 Linux: native engine, no VM

- **Decision:** On Linux the engine runs natively and rootless. A KVM microVM is an opt-in.
- **Why:** Nothing is faster than no hypervisor.

## D-020 Rootful engine inside the VM on macOS

- **Decision:** dockerd runs as root inside the guest.
- **Why:** The VM is already the security boundary on a Mac. Rootless inside the VM adds user-space networking and storage restrictions that cost speed and break some images.

## D-021 Lima and Apple's container tool are not shipped

- **Decision:** Neither is a dependency of the product.
- **Why:** Lima adds a separate Go process driven by SSH that Marsa would not control. Apple's tool is not the Docker API.
- **Allowed:** Lima may be used as a temporary bootstrap engine during early GUI development.

---

## D-022 Licence Marsa under MIT

- **Decision:** The project owner chose the MIT licence for Marsa. This resolves the licence question left open by D-017 and Q-01.
- **Consequence:** Add the project licence. Continue to track licence and redistribution obligations for each upstream component.
- **Date and task:** 2026-11-21, M0-02.

## D-023 Choose GPUI for the desktop

- **Decision:** The owner chose GPUI as the Rust desktop UI framework. This closes G1 and supersedes D-004 and D-005.
- **Why:** M1-01 reproduced Arabic caret and hit-testing defects in stock GPUI Kit input. The desktop qualification verified a Rust-backed AppKit editor inside GPUI for Arabic typing, native arrow events, pointer selection and live IME conversion. It also demonstrated an AppKit layout that exposes both the native editor and a GPUI control to macOS accessibility. [Qualification evidence](spikes/M1-06-gpui-qualification.md).
- **Consequence:** Keep direction, localization and GUI accessibility in the Rust component layer. Validate the native editor in the production window. M6-08 covers physical arrow keys, the live IME candidate panel and spoken VoiceOver reading and editing. D-015 still requires all four language and theme variants.
- **Reopen if:** The production integration cannot satisfy the Arabic editing or accessibility requirements at a maintainable cost.
- **Date and task:** 2026-11-21, M1-05.

## D-024 Use direct Rust bindings for Virtualization.framework

- **Decision:** The owner approved the generated `objc2-virtualization` bindings for the macOS backend. This closes G2.
- **Why:** Both bridges validated the full planned device list, booted the signed Alpine test VM, completed a vsock round trip, and delivered start and shutdown callbacks. The generated bindings keep the build and framework ownership in Rust. [M1-02 comparison](spikes/M1-02-vz-binding.md).
- **Consequence:** Retain the Objective-C delegate and completion blocks for their documented lifetimes. Own framework objects and their callbacks on the backend’s serial dispatch queue and keep socket I/O off that queue. M3-02 owns production configuration and boot.
- **Reopen if:** A required Virtualization.framework API is unavailable in the generated bindings or a required callback cannot meet the queue and lifetime contract.
- **Date and task:** 2026-11-21, M1-05.

## D-025 Pin the current stable Linux kernel

- **Decision:** Pin Linux 7.3.5, the current upstream stable release at the time of M2-01.
- **Why:** The owner requires current stable dependencies. Kernel.org identified 7.3.5 as its latest stable release on 2026-11-21. The build uses kernel.org’s published SHA-256, and M2-02 validates the minimal configuration and boots the result. [Upstream releases](https://www.kernel.org/releases.json).
- **Consequence:** Update the exact release and checksum together. Verify the Docker configuration requirements and boot the pinned kernel before shipping an update.
- **Date and task:** 2026-11-21, M2-01.

## D-026 Use EROFS for the guest root filesystem

- **Decision:** Pack the root tree as LZ4-compressed EROFS and attach it as a read-only virtio block device.
- **Why:** Alpine 3.24 provides `erofs-utils` 1.9.1-r0 with tar input and a full data check. Linux 7.3.5 with `CONFIG_EROFS_FS=y` and built-in compression support mounted that image directly as `/` in Virtualization.framework and returned `Read-only file system` when PID 1 attempted a write. [EROFS build documentation](https://erofs.docs.kernel.org/en/latest/mkfs.html), [reproduction](../spikes/vz-boot/README.md#read-only-erofs-root).
- **Consequence:** Build and validate the image in the pinned Linux formatter container. Record its SHA-256 and the raw arm64 boot kernel's SHA-256 in the guest manifest.
- **Date and task:** 2026-11-21, M2-04.

## D-027 Apply proxy settings at a controlled engine restart

- **Decision:** Apply DNS settings immediately in the guest. Stage Docker proxy and host-gateway changes that arrive during a running session and report that they require a restart. Configure Docker before the first engine start. M4-15 owns restart orchestration.
- **Why:** Docker 29.10.0 does not reload daemon proxies or host-gateway IPs. A restart can affect active workloads. Docker also retains system trust in some running clients. [Proxy configuration](https://docs.docker.com/engine/daemon/proxy/) · [Pinned daemon reload source](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/reload.go).
- **Consequence:** The host tracks desired settings across guest boots. Connect the restart action to the daemon lifecycle and give the user control over any workload impact. M4-08 injects compatibility aliases for standard Docker clients.
- **Date and task:** 2026-11-21, M2-07.

## D-028 Qualify native IPv4 independently of runtime and power management

- **Decision:** The owner approved proceeding with power management and packaged daemon composition using the qualified user-space fallback. Keep the strict native IPv4 acceptance test independently runnable.
- **Why:** The same Apple silicon Mac verified VZ pause and resume, guest control, and fallback egress, while native IPv4 repeatedly failed. Pausing, resuming, and synchronizing guest time require VZ lifecycle and vsock support. The packaged runtime can validate its bundled guest and start local Docker using the boot selector.
- **Consequence:** M3-16 owns common device support. M3-11 and M3-12 depend on those capabilities and M3-12 uses M3-15 for automatic egress selection. M3-04 keeps native DHCP, IPv4 address, default gateway, and verified HTTPS as its own acceptance gate.
- **Reopen if:** The fallback and native attachments require different stop, pause, resume or guest control semantics.
- **Date and task:** 2026-11-22, M3-16.
---

## D-031 Scope development file sync with Compose Watch

- **Decision:** Support explicitly selected development source directories through Compose Watch sync into the container filesystem.
- **Why:** The production Apple virtiofs configuration reflected the updated bytes from a Mac atomic save without generating an inotify event. With signed Compose 5.8.0 on the same backend, an atomic save produced native Linux create, modify and close-write notifications. The fixture confirmed initial sync, exact updated contents, and exclusion of ignored and out-of-scope files.
- **Consequence:** Show the scoped configuration in the development guide and keep the ordinary bind-mount workflow for shared files. Qualify a common development server in M4-19. [Compose Watch](https://docs.docker.com/compose/how-tos/file-watch/)
- **Reopen if:** A supported Apple virtiofs interface provides host-origin notifications with the same share isolation.
- **Date and task:** 2026-11-22, M4-06.

---

## Template

```markdown
## D-NNN Title

- **Decision:**
- **Why:**
- **Rejected:**
- **Consequence:**
- **Reopen if:**
- **Date and task:**
```


## D-029 Verify updates with TUF

- **Decision:** Keep the updater under `marsad` so interactive and headless clients share one authorization, VM shutdown and rollback path. Use TUF metadata and an audited, maintained Rust verifier for signatures, expiry, rollback detection and secure key rotation. Require a separately verified Apple Developer ID signature and notarization for the app before any install. [Manifest and key procedures](reference/update-manifest.md).
- **Why:** Sparkle 2.10.0 is maintained and supplies signed feeds, trust checks and update installation. Its separately distributed command-line tool demonstrates headless operation. Marsa still needs daemon-controlled shutdown, restart and rollback after real engine health is established, shared by the UI and CLI. TUF supplies standard signed metadata and key recovery without owning the VM or bundle replacement. [Sparkle release](https://github.com/sparkle-project/Sparkle/releases/tag/2.10.0), [Sparkle command-line tool](https://sparkle-project.org/documentation/sparkle-cli/), [TUF specification](https://theupdateframework.github.io/specification/v1.0.33/).
- **Consequence:** M10-04 owns safe archive extraction, verified bundle replacement and the rollback transaction. Its dependency must pass the production policy. Establish project custody and Apple signing under Q-04 before publishing a production root or update.
- **Date and task:** 2026-11-22, M10-03.

## D-030 Scope build SSH agent access to Buildx sessions

- **Decision:** Use upstream Buildx SSH sessions for builds. Keep explicit container-mounted agent forwarding in M4-17.
- **Why:** Buildx resolves the host agent when building the client session and BuildKit supplies a mount to the requesting step. The session affordance is sufficient for a private Git clone. The Docker byte proxy cannot infer which other guest workload, if any, the user authorized from that build session. A fixed guest listener would be visible across unrelated guest workloads with mount access. [Pinned Buildx setup](https://github.com/docker/buildx/blob/v0.39.1/commands/build.go#L1016-L1024) · [Pinned BuildKit provider](https://github.com/moby/buildkit/blob/v0.35.0/session/sshforward/sshprovider/agentprovider.go).
- **Consequence:** Validate the unmodified Docker CLI and Buildx against a private fixture. Require a separate host-side authorization and revocation model before adding a socket for regular containers.
- **Reopen if:** The pinned CLI or engine changes the Buildx session contract.
- **Date and task:** 2026-11-22, M4-13.


## D-032 Use IBM Plex for desktop typography

- **Decision:** Use the Figma Foundations' IBM Plex typography. Bundle unmodified IBM Plex Sans and IBM Plex Sans Arabic for UI text and IBM Plex Mono for Latin technical text.
- **Why:** The dedicated Arabic faces supply the same regular, medium and semibold roles as the Latin design. HarfBuzz verified joining in each Arabic weight, and CoreText read the original native files on macOS.
- **Consequence:** Verify the [pinned font files and licence notice](../assets/fonts/README.md) at packaging, and use the Arabic face where Mono lacks a glyph. M6-02 owns GPUI registration and rendering.
- **Date and task:** 2026-11-22, M6-13.

## D-033 Keep the command-line interface in English

- **Decision:** Use English for the Marsa CLI. Offer English and Arabic language selection in the desktop GUI under D-015.
- **Why:** The owner clarified that Arabic support applies to the desktop interface.
- **Consequence:** CLI help, messages, confirmation prompts and completion descriptions use English under every shell locale. Do not expose a CLI language override. Preserve machine response contracts and user-supplied Unicode data.
- **Date and task:** 2026-11-22, M5-07.

## D-034 Cache Docker inventory in the daemon

- **Decision:** `marsad` owns a durable Docker inventory cache under `~/.marsa/state/`. Owner clients (GUI, CLI, MCP) read the last snapshot first and never keep a private inventory database. Background refresh and live Docker reads update the cache; per-container live stats stay uncached. Version 1 stores sections in an owner-only JSON file; SQLite remains the preferred durable format once the workspace dependency graph accepts it.
- **Why:** D-003 makes the headless daemon the system of record. Overview launch was doing a cold, multi-call Docker fan-out on every open, which contradicts the GUI start-up rule (first frame from a snapshot, no client polling). A daemon cache keeps the CLI and MCP as capable as the GUI. Bundled `rusqlite` currently conflicts with the vendored `hashbrown` used by Prost.
- **Consequence:** Host API minor 14 adds `prefer_cache` and `observed_at_unix_millis` on `Stats.DockerOverview`. External engines remain client-side until a later task. A follow-up may swap the JSON file for SQLite without changing the API.
- **Date and task:** 2026-11-25, M4-26.

## D-035 The project owner reviews Arabic terminology

- **Decision:** The project owner will review the Arabic desktop copy and the terminology glossary.
- **Why:** The owner volunteered to perform the review for Q-06.
- **Consequence:** M6-04 prepares the glossary. M7-14 records the owner's review and any corrections alongside the four-variant audit.
- **Date and task:** 2026-11-25, M0-10.

## D-036 Offer the optional Docker socket link in Settings

- **Decision:** Offer the optional `/var/run/docker.sock` link in desktop Settings only. Leave it off by default.
- **Why:** The owner chose the settings-only placement for Q-08.
- **Consequence:** M4-04 provides an auditable one-time administrator action and matching removal for the owner socket. First-run setup uses the normal Docker context.
- **Date and task:** 2026-11-25, M0-10.

## D-037 Offer opt-in crash reporting

- **Decision:** Add opt-in crash reporting under M9-12. Reporting is off by default.
- **Why:** The owner chose opt-in crash reporting for Q-10.
- **Consequence:** The implementation documents report fields, the destination and retention before consent. Reports follow the [privacy rules](11-security.md#privacy) and the outbound-connection test covers the reporting path.
- **Date and task:** 2026-11-25, M0-10.

## D-038 Bind published ports to loopback by default

On 2026-11-26 the owner resolved Q-07. Marsa binds published ports on macOS to IPv4 and IPv6 loopback by default. LAN access requires explicit opt-in. This keeps development services private when the Mac changes networks. M4-09 implements and qualifies the host listeners.
