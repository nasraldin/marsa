# 06 Networking

Networking has to do four things: give containers internet access, make published ports reachable on `localhost`, let containers reach the host, and keep working on VPNs and corporate networks.

## Decisions

| Item | Decision | Status |
|---|---|---|
| Default mode | Kernel-level NAT provided by macOS (fast path) | Decided |
| Fallback mode | User-space network stack, chosen automatically when NAT is blocked | Decided |
| Published ports | A Rust forwarder inside `marsad` | Decided |
| Mode selection | `auto` by default, with a manual override | Decided |

Planning research found the trade-off behind this: kernel NAT keeps packets in the OS network stack and is the fastest path, while a common open-source user-space stack (gvisor-tap-vsock) reports about 1.6 to 2.3 Gbit/s in its own test but keeps working in environments where the NAT path fails.

## Network modes

```mermaid
flowchart TB
    subgraph nat["Mode: nat (default, fast)"]
        direction LR
        c1["Containers"] --> b1["docker0 bridge<br/>(guest)"] --> e1["eth0 (guest)<br/>virtio-net"] --> v1["macOS NAT<br/>(vmnet)"] --> i1["Internet"]
    end

    subgraph us["Mode: userspace (compatible)"]
        direction LR
        c2["Containers"] --> b2["docker0 bridge<br/>(guest)"] --> e2["eth0 (guest)<br/>virtio-net"] --> s2["User-space stack<br/>in a marsad helper"] --> k2["Host sockets<br/>(VPN-aware)"] --> i2["Internet"]
    end
```

| | `nat` | `userspace` |
|---|---|---|
| How traffic leaves | Forwarded by macOS as NATed packets | Re-originated by a host process as ordinary sockets |
| Speed | Highest | Lower |
| VPNs and strict firewalls | Can fail when the VPN blocks forwarded traffic | Works, because traffic looks like it comes from a normal app |
| Guest IP reachable from host | Yes | No; ports are forwarded through the stack |
| Framework attachment | NAT attachment | File-handle (datagram socket) attachment |

**Verify:** the exact Virtualization.framework attachment classes and whether the NAT attachment needs any extra entitlement on the minimum macOS version.

### The user-space stack

**Proposed:** run gvisor-tap-vsock's `gvproxy` as a bundled helper process for version 1. It is proven, it is used by Podman and Lima, and it already provides DHCP, DNS and port forwarding. A pure-Rust stack is a possible later optimisation, not a version 1 goal.

## Automatic mode selection

```mermaid
flowchart TD
    start["VM reaches Ready, or network changed, or host woke"] --> probe["Agent probes: resolve a name,<br/>open TCP to a known registry"]
    probe --> ok{"Probe succeeds<br/>in nat mode?"}
    ok -- yes --> keep["Stay in nat"]
    ok -- no --> hostok{"Same probe succeeds<br/>from the host?"}
    hostok -- no --> offline["Host is offline: stay in nat,<br/>show offline state"]
    hostok -- yes --> switch["Switch to userspace mode<br/>(restart network device or VM)"]
    switch --> note["Publish event: mode changed and why"]
```

Rules:

- The user can pin a mode in settings; `auto` never overrides a pinned mode.
- A mode switch is announced in the GUI and the event log, with the reason.
- Probe targets are configurable, and no probe sends identifying data.

## Published ports

When a container publishes a port, it must be reachable at `localhost:<port>` on the Mac.

```mermaid
sequenceDiagram
    autonumber
    participant E as dockerd
    participant A as marsa-agent
    participant D as marsad forwarder
    participant B as Browser on the Mac
    participant C as Container

    E->>A: Event: container started with 0.0.0.0:8080->80/tcp
    A->>D: PortOpened(8080, tcp, bind 0.0.0.0)
    D->>D: Bind 127.0.0.1:8080 (or 0.0.0.0 if allowed by settings)
    B->>D: Connect to localhost:8080
    D->>C: Forward to guest (direct TCP in nat mode, through the stack in userspace mode)
    C-->>B: Response
    E->>A: Event: container stopped
    A->>D: PortClosed(8080, tcp)
    D->>D: Close listener
```

Forwarder requirements:

- TCP and UDP.
- IPv4 and IPv6 loopback.
- **Bind address policy (Proposed):** ports published on `0.0.0.0` are bound to `127.0.0.1` on the Mac by default, so containers are not exposed to the local network unless the user enables "allow LAN access". This is safer than Docker Desktop's default and must be stated clearly in the UI.
- Privileged ports (below 1024): **Verify** current macOS behaviour for unprivileged binding; if binding fails, report it clearly.
- Port conflicts produce a clear error naming the process that holds the port when it can be determined.
- The forwarder is zero-copy where the platform allows and must not be the bottleneck in the network benchmark.

## Container to host

| Name | Resolves to | Purpose |
|---|---|---|
| `host.docker.internal` | The host as seen from the guest | Compatibility with existing Compose files |
| `gateway.docker.internal` | The guest's default gateway | Compatibility |
| `host.marsa.internal` | Same as `host.docker.internal` | Marsa's own name |

The agent configures these names in the engine's DNS or `/etc/hosts` injection (**Proposed:** through dockerd's `host-gateway` support and an extra hosts entry).

## DNS

```mermaid
flowchart LR
    c["Container"] --> ed["Docker embedded DNS<br/>127.0.0.11"]
    ed --> gr["Guest resolver config<br/>(written by marsa-agent)"]
    gr -->|nat mode| hd["Host DNS via NAT gateway"]
    gr -->|userspace mode| ud["Stack's DNS forwarder"]
    hd --> up["Upstream resolvers,<br/>including VPN split DNS"]
    ud --> up
```

- `marsad` reads the host's resolver configuration and pushes it to the agent at boot and on every network change.
- Split DNS from VPNs must keep working. In `userspace` mode the host resolver is used directly, which honours it.

## Proxies and certificates

Corporate networks were identified in planning as a top reason Docker alternatives fail.

| Need | Design |
|---|---|
| HTTP and HTTPS proxy | `marsad` reads system proxy settings (and manual overrides) and the agent configures dockerd and BuildKit with them |
| Custom CA certificates | `marsad` exports the certificates the user trusts in the macOS keychain and the agent installs them into the guest trust store before dockerd starts |
| Registry mirrors and insecure registries | Exposed in settings and written to the engine configuration |
| Proxy for containers | Optional injection of proxy variables into containers, off by default |

## IPv6

**Proposed:** IPv6 inside Docker networks follows the engine's configuration. IPv6 to the internet depends on the mode and on what macOS NAT provides. Treat full IPv6 egress as best effort in version 1 and document the behaviour that the tests observe.

## Sleep, wake and network change

- On wake and on network change, `marsad` re-pushes DNS and proxy settings, re-runs the mode probe and re-validates listeners.
- Long-lived connections that died during sleep are closed cleanly so clients reconnect.

## Test matrix

| Scenario | Must work |
|---|---|
| Plain home network | Pull, run, publish, `host.docker.internal` |
| Captive or offline network | Clear offline state, no hangs |
| Full-tunnel VPN | Pull and run after automatic switch to `userspace` |
| Split-tunnel VPN with split DNS | Internal names resolve from containers |
| Corporate proxy with TLS interception | Pull succeeds after CA import |
| Sleep for an hour, then wake | Ports and DNS work without restarting |
| Wi-Fi to Ethernet switch | Connectivity restored without restarting |
