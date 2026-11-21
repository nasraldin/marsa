# 15 Decision records

Each record states what was decided, why, what was rejected and what would reopen it. Records are numbered and never rewritten; a changed decision gets a new record that supersedes the old one.

To add a record: copy the template at the bottom, take the next number, and link it from the doc it affects.

## Index

| # | Decision | Status |
|---|---|---|
| D-001 | Reuse the Docker engine unmodified | Accepted |
| D-002 | Rust for all host code | Accepted |
| D-003 | Headless daemon; GUI is an optional client | Accepted |
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
| D-027 | Apply proxy settings at a controlled engine restart | Accepted |

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

## D-027 Apply proxy settings at a controlled engine restart

- **Decision:** Apply DNS settings immediately in the guest. Stage Docker proxy and host-gateway changes that arrive during a running session and report that they require a restart. Configure Docker before the first engine start. M4-15 owns restart orchestration.
- **Why:** Docker 29.10.0 does not reload daemon proxies or host-gateway IPs. A restart can affect active workloads. Docker also retains system trust in some running clients. [Proxy configuration](https://docs.docker.com/engine/daemon/proxy/) · [Pinned daemon reload source](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/reload.go).
- **Consequence:** The host tracks desired settings across guest boots. Connect the restart action to the daemon lifecycle and give the user control over any workload impact. M4-08 injects compatibility aliases for standard Docker clients.
- **Date and task:** 2026-11-21, M2-07.

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
