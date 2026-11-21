# Marsa

Marsa (Arabic: مرسى, "harbour") is a fast, lightweight, open-source desktop app for running Docker containers. It replaces the heavy parts of Docker Desktop (the Electron GUI and the general-purpose VM stack) and keeps the open-source Docker engine unmodified.

**Status:** workspace foundation in progress. See the [implementation plan](docs/plan/README.md) for task status.

## What Marsa is

- The unmodified Docker engine (dockerd, containerd, runc, BuildKit, Compose) running in a minimal Linux VM.
- A small Rust host daemon (`marsad`) that owns the VM, networking, file sharing and the Docker socket.
- A desktop GUI built with Tauri 2, in English (US) and Arabic (Modern Standard), with full left-to-right and right-to-left layouts and light and dark themes.
- A CLI (`marsa`) and an MCP server so AI coding agents can drive containers safely.

## First release target

| Item | Decision |
|---|---|
| Platform | macOS on Apple silicon only |
| Container images | linux/arm64 only (no Rosetta, no x86 emulation) |
| Hypervisor | Apple Virtualization.framework (libkrun evaluated as a second backend) |
| Guest | Alpine-based minimal root filesystem, read-only, with a separate data disk |
| GUI | Tauri 2 with a system webview |
| Licence | [MIT](LICENSE) |

Windows (WSL2) and Linux (native engine) follow after the macOS release, on the same daemon.

## Documentation

Start with [docs/README.md](docs/README.md). It indexes the architecture, every component design, the decision records and the task-by-task implementation plan.

Coding agents: read [AGENTS.md](AGENTS.md) before doing anything.

See the [contribution guide](CONTRIBUTING.md), [Code of Conduct](CODE_OF_CONDUCT.md), and [security policy](SECURITY.md).

## Naming

| Thing | Name |
|---|---|
| Product | Marsa |
| GUI app | `Marsa.app` |
| Host daemon | `marsad` |
| CLI | `marsa` |
| Guest init | `marsa-init` |
| Guest agent | `marsa-agent` |

Marsa is Docker-compatible. It is not affiliated with Docker, Inc., and "Docker" is a trademark of Docker, Inc.
