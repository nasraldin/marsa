# 01 Vision and scope

## Problem

Docker Desktop is slow and heavy for three reasons that have nothing to do with the Docker engine itself:

1. The GUI is an Electron app that holds hundreds of megabytes while idle.
2. The Linux VM is a general-purpose system that reserves memory and rarely gives it back.
3. File sharing and networking between the host and the VM add latency to everyday work.

The engine (dockerd, containerd, runc, BuildKit, Compose) is open source and works well. Marsa keeps it and replaces everything around it.

## Vision

A container desktop that starts in seconds, idles near zero, feels native, speaks Arabic as well as it speaks English, and can be driven safely by AI coding agents.

## Principles

| Principle | What it means in practice |
|---|---|
| Reuse the engine, never fork it | The Docker engine binaries run unmodified. Marsa tracks upstream releases. |
| The GUI is optional | The daemon runs headless. Closing the window leaves nothing running except the daemon and, if containers are running, the VM. |
| One Rust daemon | All host logic lives in `marsad`. The GUI, CLI and MCP server are clients of the same typed API. |
| Native virtualization only | Each OS uses its own hypervisor. No QEMU, no emulation. |
| Modern platforms only | No legacy OS versions, no Intel Macs. |
| Bilingual desktop | The desktop supports English (US) and Modern Standard Arabic with mirrored layouts. The command-line interface uses English. |
| Secure by default | Least privilege, explicit folder sharing, signed updates, and permission tiers for agents. |
| Correct and complete | No demo-quality features. Each feature ships in every language and theme variant, with tests. |

## Scope of version 1

**Decided.** Version 1 is macOS on Apple silicon, open source.

### In scope

- Run, build and manage linux/arm64 containers with the standard `docker` CLI, Compose and Buildx.
- A Docker-compatible socket so IDEs, Testcontainers and Dev Containers work unchanged.
- `marsad`, the `marsa` CLI, the Marsa GUI and the MCP server.
- GUI features listed under "Must have" in [08 GUI](08-gui.md#feature-priorities).
- English (US) and Arabic (MSA), left-to-right and right-to-left, light and dark themes.
- Signed and notarized distribution with signed auto-update.

### Out of scope for version 1

| Item | Status |
|---|---|
| Windows and Linux hosts | Planned after version 1. See [14 Future platforms](14-future-platforms.md). |
| x86 (amd64) images on Apple silicon | **Decided:** supported through macOS 27 Intel Linux translation (D-039). |
| Intel Macs | **Decided:** not supported. |
| Kubernetes | **Decided:** later. |
| AI assistant inside the GUI | Later. The MCP server is in version 1; the in-app assistant is not. |
| Vulnerability scanning, volume backup, local domains with HTTPS, dev containers UI, extensions | Later. |
| Windows containers | Not planned. |

## Target platform

| Item | Value | Status |
|---|---|---|
| Hardware | Apple silicon (arm64) | Decided |
| Minimum macOS | macOS 27 | Decided in [Q-02](16-risks-and-open-questions.md#resolved-platform-and-runner-questions). |
| Container architecture | linux/arm64 | Decided |

## Success measures

These are the targets the benchmarks in [13 Testing and benchmarks](13-testing-and-benchmarks.md) must report against. The numbers are goals set during planning, not measurements.

| Measure | Target | Status |
|---|---|---|
| Cold start to first container running | Under 3 seconds | Proposed |
| GUI idle memory | Under 100 MB | Proposed |
| Daemon idle memory, VM stopped | Under 30 MB | Proposed |
| VM idle memory, no containers | Under 400 MB, and returned to the host after load | Proposed |
| Bind-mount file read throughput | Within 2x of native APFS | Proposed |
| GUI window closed | Zero GUI processes | Decided |

## Users

- Developers on Apple silicon Macs who run containers daily and find Docker Desktop heavy.
- Arabic-speaking developers and teams who want a fully Arabic tool.
- Teams using AI coding agents that need controlled access to containers.

## Licence and trademark

- **Decided:** Marsa is open source.
- **Open:** the exact licence. Apache-2.0 is recommended because the Docker engine (Moby), gpui-component and libkrun use it. See [open questions](16-risks-and-open-questions.md).
- Marsa may describe itself as "Docker-compatible". It must not use "Docker" in its name, logo or binary names beyond what compatibility requires (for example the bundled upstream `docker` CLI). This is not legal advice; have it reviewed.
