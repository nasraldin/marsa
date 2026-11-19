# 17 Glossary

| Term | Meaning |
|---|---|
| Allowlist | The list of host folders the user has agreed to share with the VM |
| Backend | An implementation of the Hypervisor trait: `vz`, `krun`, later `wsl2`, `hcs`, `native`, `kvm` |
| Balloon | A virtual device that lets the host ask the guest to give memory back |
| BuildKit | The image builder inside the Docker engine, used through `docker buildx` |
| containerd | The container runtime manager used by dockerd |
| Control channel | The vsock connection between `marsad` and `marsa-agent` |
| Data disk | The sparse disk image holding images, containers and volumes |
| Direction layer | The part of `marsa-ui` that resolves start and end to left and right for the current language |
| dockerd | The Docker engine daemon; runs in the guest, unmodified |
| Docker context | The Docker CLI's named configuration for which engine to talk to |
| Engine | dockerd, containerd, runc and BuildKit together |
| Fluent | A translation system with full plural-rule support |
| GPUI | The GPU-rendered Rust UI framework from the Zed editor |
| gpui-component | A component library for GPUI from Longbridge |
| Guest | The Linux system inside the VM |
| HCS | Host Compute Service, the Windows API that WSL2 is built on |
| Host | The user's Mac |
| Hypervisor.framework | Apple's low-level virtualization API, used by libkrun |
| Hypervisor trait | The Rust interface every VM backend implements |
| libkrun | An open-source Rust virtual machine monitor |
| LTR, RTL | Left-to-right, right-to-left |
| `marsa` | The Marsa command-line tool |
| `marsad` | The Marsa host daemon |
| `marsa-agent` | The helper inside the guest that the daemon controls |
| `marsa-init` | PID 1 inside the guest |
| `marsa-ui` | Marsa's design system crate |
| MCP | Model Context Protocol, the interface AI agents use to call tools |
| MSA | Modern Standard Arabic |
| NAT mode | Networking through macOS's kernel-level address translation |
| Notarization | Apple's check of signed software distributed outside the App Store |
| Protected paths | Credential directories that can never be shared |
| runc | The low-level container runtime |
| SBOM | Software bill of materials: the list of everything shipped |
| Spike | A short, time-boxed experiment that answers a specific question |
| Typed API | The gRPC API `marsad` serves to the GUI, CLI and MCP server |
| User-space mode | Networking through a network stack running as a host process |
| virtiofs | A shared-filesystem protocol between host and guest |
| Virtualization.framework | Apple's high-level VM API, used by the `vz` backend |
| vsock | A socket type for host-to-guest communication without networking |
| WSL2 | Windows Subsystem for Linux 2, a lightweight Hyper-V VM |
