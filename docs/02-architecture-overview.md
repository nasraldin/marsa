# 02 Architecture overview

This page shows the whole system. Each box has its own design doc.

## 1. System context

Who and what talks to Marsa.

```mermaid
flowchart LR
    dev["Developer"]
    agent["AI coding agent"]
    ide["IDEs, Testcontainers,<br/>Dev Containers, CI scripts"]
    reg["Container registries"]
    upd["Marsa update server"]

    subgraph marsa["Marsa on the Mac"]
        gui["Marsa GUI"]
        cli["marsa CLI"]
        dcli["docker CLI + Compose + Buildx"]
        mcp["MCP server"]
        d["marsad host daemon"]
        vm["Linux VM with Docker engine"]
    end

    dev --> gui
    dev --> cli
    dev --> dcli
    agent --> mcp
    ide -->|Docker socket| d
    gui --> d
    cli --> d
    dcli -->|Docker socket| d
    mcp --> d
    d --> vm
    vm -->|pull and push| reg
    d -->|signed updates| upd
```

## 2. Layered architecture

This is the diagram from the planning whiteboard, narrowed to the macOS release. Colours follow the whiteboard: blue is new code, green is reused upstream, yellow is the native OS layer.

```mermaid
flowchart TB
    subgraph clients["Clients"]
        gui["Marsa GUI (optional)<br/>GPUI + gpui-component, all Rust"]
        dcli["docker CLI + Compose + Buildx<br/>upstream, unchanged"]
        agents["AI coding agents<br/>through MCP"]
        mcli["marsa CLI"]
    end

    daemon["Host daemon marsad: Rust, headless, tray icon<br/>Docker socket proxy · typed local API · MCP server<br/>one codebase, one VM backend per OS"]

    subgraph native["Native virtualization (macOS, Apple silicon)"]
        vz["Virtualization.framework<br/>virtiofs · vsock · NAT<br/>default backend"]
        krun["libkrun on Hypervisor.framework<br/>second backend, benchmarked"]
    end

    subgraph guest["Linux guest (arm64)"]
        init["marsa-init + marsa-agent (Rust)"]
        engine["Docker engine, unmodified (Go)<br/>dockerd · containerd · runc · BuildKit"]
        os["Alpine-based minimal rootfs, read-only<br/>own minimal kernel · separate data disk"]
    end

    gui --> daemon
    dcli --> daemon
    agents --> daemon
    mcli --> daemon
    daemon --> vz
    daemon -.-> krun
    vz --> guest
    krun -.-> guest

    classDef newcode fill:#dbeafe,stroke:#1d4ed8,color:#0f172a
    classDef upstream fill:#dcfce7,stroke:#15803d,color:#0f172a
    classDef nativeos fill:#fef9c3,stroke:#a16207,color:#0f172a
    class gui,daemon,mcli,init newcode
    class dcli,engine,os upstream
    class vz,krun nativeos
```

## 3. Processes and channels

What actually runs on the machine, and how the pieces talk.

```mermaid
flowchart LR
    subgraph host["macOS host (user session, no root)"]
        gui["Marsa.app<br/>GUI process"]
        cli["marsa"]
        dcli["docker"]
        d["marsad"]
        fwd["port forwarder<br/>(inside marsad)"]
        sock[("~/.marsa/run/docker.sock")]
        api[("~/.marsa/run/marsad.sock")]
    end

    subgraph vm["Linux VM"]
        agent["marsa-agent"]
        dockerd["dockerd"]
        ctr["containers"]
    end

    gui -->|typed API| api
    cli -->|typed API| api
    api --- d
    gui -->|Docker Engine API| sock
    dcli -->|Docker Engine API| sock
    sock --- d
    d ==>|vsock: control| agent
    d ==>|vsock: Docker API stream| dockerd
    d -.->|virtiofs: shared folders| ctr
    fwd -->|TCP and UDP| ctr
    dockerd --> ctr
```

| Channel | Transport | Carries |
|---|---|---|
| Typed API | Unix socket, gRPC | VM lifecycle, settings, sharing, diagnostics, events |
| Docker socket | Unix socket, HTTP | The standard Docker Engine API, proxied byte-for-byte to dockerd |
| Control | vsock | Time sync, port events, mounts, certificates, shutdown, stats |
| Docker API stream | vsock | The proxied Docker Engine API |
| Shared folders | virtiofs | Bind-mounted host directories |
| Published ports | TCP and UDP | Traffic from `localhost` on the Mac to containers |

## 4. Key flow: `docker run -p 8080:80 -v ~/app:/app nginx`

```mermaid
sequenceDiagram
    autonumber
    participant U as docker CLI
    participant D as marsad
    participant A as marsa-agent
    participant E as dockerd
    participant C as container

    U->>D: POST /containers/create (Docker socket)
    D->>D: Check ~/app is in the shared-folder allowlist
    alt VM stopped
        D->>D: Start VM, wait for agent ready
    end
    D->>E: Forward request over vsock
    E->>C: Create and start container
    E-->>D: 201 Created
    D-->>U: 201 Created
    E->>A: Container started, port 80 published as 8080
    A->>D: Port event (8080 tcp)
    D->>D: Listen on 127.0.0.1:8080, forward to guest
    Note over U,C: Browser on the Mac opens localhost:8080
```

## 5. Component map

| Component | Language | Runs on | Doc |
|---|---|---|---|
| `marsad` | Rust | Host | [03](03-host-daemon.md) |
| VM backends (`vz`, `krun`) | Rust, with a thin Apple bridge | Host | [04](04-virtualization-macos.md) |
| Guest image (kernel, rootfs) | Build scripts | Guest | [05](05-guest-image.md) |
| `marsa-init`, `marsa-agent` | Rust | Guest | [05](05-guest-image.md) |
| Networking and port forwarder | Rust | Host and guest | [06](06-networking.md) |
| File sharing and data disk | Rust | Host and guest | [07](07-file-sharing-and-storage.md) |
| Marsa GUI | Rust (GPUI) | Host | [08](08-gui.md) |
| `marsa` CLI and Docker compatibility | Rust, bundled upstream binaries | Host | [09](09-cli-and-docker-compat.md) |
| MCP server | Rust | Host | [10](10-mcp-server.md) |
| Docker engine | Go, upstream | Guest | [05](05-guest-image.md) |

## 6. Repository layout

**Proposed.** A single Cargo workspace.

```text
marsa/
├── AGENTS.md
├── README.md
├── Cargo.toml                  # workspace
├── crates/
│   ├── marsa-core/             # shared types, config, errors, paths
│   ├── marsa-proto/            # protobuf: host API and guest control protocol
│   ├── marsa-vm/               # Hypervisor trait and VM model
│   ├── marsa-vm-vz/            # Virtualization.framework backend
│   ├── marsa-vm-krun/          # libkrun backend
│   ├── marsa-net/              # port forwarder, DNS, network modes
│   ├── marsa-share/            # shared-folder allowlist, virtiofs mapping
│   ├── marsa-dockerproxy/      # Docker socket proxy and policy hooks
│   ├── marsad/                 # host daemon binary
│   ├── marsa-client/           # typed API client used by GUI, CLI, MCP
│   ├── marsa-cli/              # `marsa` binary
│   ├── marsa-mcp/              # MCP server
│   ├── marsa-ui/               # design tokens, direction, i18n, components
│   ├── marsa-gui/              # GUI binary
│   ├── marsa-agent/            # guest agent (linux/arm64, musl)
│   └── marsa-init/             # guest init (linux/arm64, musl)
├── guest/
│   ├── kernel/                 # kernel config and build
│   ├── rootfs/                 # Alpine-based rootfs recipe
│   └── image/                  # disk image assembly
├── packaging/
│   └── macos/                  # app bundle, entitlements, signing, notarization
├── locales/                    # en-US and ar translation files
├── xtask/                      # build, package and release automation
├── tests/                      # integration, conformance, benchmarks
└── docs/
```

## 7. Crate dependencies

```mermaid
flowchart TD
    core["marsa-core"]
    proto["marsa-proto"]
    vm["marsa-vm"]
    vz["marsa-vm-vz"]
    krun["marsa-vm-krun"]
    net["marsa-net"]
    share["marsa-share"]
    dproxy["marsa-dockerproxy"]
    d["marsad"]
    client["marsa-client"]
    cli["marsa-cli"]
    mcp["marsa-mcp"]
    ui["marsa-ui"]
    gui["marsa-gui"]
    agent["marsa-agent"]
    init["marsa-init"]

    proto --> core
    vm --> core
    vz --> vm
    vz --> core
    vz --> proto
    krun --> vm
    net --> core
    share --> core
    dproxy --> core
    dproxy --> share
    d --> vz
    d --> krun
    d --> net
    d --> share
    d --> dproxy
    d --> proto
    client --> proto
    cli --> client
    mcp --> client
    gui --> client
    gui --> ui
    ui --> core
    agent --> proto
    init --> core
```

Rules the dependency graph enforces:

- The GUI, CLI and MCP server depend only on `marsa-client`. They never link VM, network or proxy code.
- Guest crates (`marsa-agent`, `marsa-init`) never depend on host-only crates.
- `marsa-vm` defines the trait. Backends depend on it, never the reverse.

## 8. Technology choices

| Area | Choice | Status |
|---|---|---|
| Host language | Rust | Decided |
| Engine | Upstream Docker engine, unmodified | Decided |
| GUI framework | GPUI with gpui-component | Decided in [D-023](15-decisions.md#d-023-choose-gpui-for-the-desktop) |
| Hypervisor (macOS) | Virtualization.framework by default; libkrun benchmarked | Decided |
| Guest base | Alpine Linux | Decided |
| Host API | gRPC over a Unix socket (tonic, prost) | Proposed |
| Guest control protocol | Protobuf messages over vsock | Proposed |
| Async runtime | Tokio | Proposed |
| Docker API client | bollard | Proposed |
| Translations | Fluent, for Arabic plural rules | Proposed; verify against gpui-component's i18n |
