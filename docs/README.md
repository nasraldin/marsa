# Marsa documentation

This folder is the single source of truth for what Marsa is, how it is built and in what order. It was written from the planning sessions that fixed the architecture, and it is meant to be read by both people and coding agents.

## How to read these docs

| If you want to | Read |
|---|---|
| Understand the product and its limits | [01 Vision and scope](01-vision-and-scope.md) |
| See the whole system on one page | [02 Architecture overview](02-architecture-overview.md) |
| Work on a specific part | The component doc for that part (03 to 12) |
| Know why something was chosen | [15 Decision records](15-decisions.md) |
| Know what is still uncertain | [16 Risks and open questions](16-risks-and-open-questions.md) |
| Pick up work | [Implementation plan](plan/README.md) and the task files next to it |

## User guides

[English](user/README.md) · [العربية](user/ar/README.md)

## Contents

### Product and architecture

1. [Vision and scope](01-vision-and-scope.md)
2. [Architecture overview](02-architecture-overview.md)

### Component designs

3. [Host daemon (`marsad`)](03-host-daemon.md)
4. [Virtualization on macOS](04-virtualization-macos.md)
5. [Guest image](05-guest-image.md)
6. [Networking](06-networking.md)
7. [File sharing and storage](07-file-sharing-and-storage.md)
8. [GUI](08-gui.md)
9. [CLI and Docker compatibility](09-cli-and-docker-compat.md)
10. [MCP server](10-mcp-server.md)
11. [Security](11-security.md)
12. [Build, release and update](12-build-release-update.md)
13. [Testing and benchmarks](13-testing-and-benchmarks.md)
14. [Future platforms: Windows and Linux](14-future-platforms.md)

### Decisions and risks

15. [Decision records](15-decisions.md)
16. [Risks and open questions](16-risks-and-open-questions.md)
17. [Glossary](17-glossary.md)

### Implementation plan

- [Plan overview, milestones and dependency graph](plan/README.md)
- Task files, one per milestone: [M0](plan/M0-foundation.md), [M1](plan/M1-risk-spikes.md), [M2](plan/M2-guest-image.md), [M3](plan/M3-vm-and-daemon.md), [M4](plan/M4-docker-integration.md), [M5](plan/M5-cli.md), [M6](plan/M6-gui-foundation.md), [M7](plan/M7-gui-features.md), [M8](plan/M8-mcp.md), [M9](plan/M9-hardening.md), [M10](plan/M10-release.md), [M11](plan/M11-performance.md)

## Status labels used in these docs

Every significant statement carries one of three labels, so a reader knows how much to trust it.

| Label | Meaning |
|---|---|
| **Decided** | Agreed in planning. Change it only through a new decision record. |
| **Proposed** | A design made to fill in the detail of a decision. It is the default, and the implementing task may refine it with a short note in the decision records. |
| **Verify** | Depends on an external fact (an API, a library capability, a platform behaviour) that was not confirmed during planning. The implementing task must check it against current upstream documentation before relying on it. |

## Diagrams

All diagrams are Mermaid and render on GitHub. The original planning whiteboard, with the first architecture sketch, the GUI framework comparison, the feature priorities and the gap list, is at <https://claude.ai/artifact/LuW1DsHWqqRHTuH42uLEUV> (private to the project owner unless shared).
