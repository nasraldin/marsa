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
| VPNs and strict firewalls | Can fail when the VPN blocks forwarded traffic | Uses ordinary host sockets and is subject to VPN and process-filter policy |
| Guest IP reachable from host | Yes | No; ports are forwarded through the stack |
| Framework attachment | NAT attachment | File-handle (datagram socket) attachment |

**Verify:** the exact Virtualization.framework attachment classes and whether the NAT attachment needs any extra entitlement on the minimum macOS version.

### The user-space stack

The VZ backend supports an opt-in helper built from the signed and checksum-pinned
gvisor-tap-vsock v0.9.0 source. Marsa runs its own small entry point over an inherited
connected datagram socket. The helper provides IPv4 DHCP and ordinary host-socket
egress. Conventional guest DNS uses the macOS scoped resolver. Each activation
has its own process, bounded readiness, guest frame validation, and egress limits.
The helper has no listening host port. The default alias to host loopback is absent.
The helper rejects host interface addresses, loopback and metadata destinations.
A helper exit publishes a network event and leaves guest control alive.
[Build and verification](../network-helper/README.md).

## Automatic mode selection

The local engine and vsock control remain available when the guest has no
upstream link or DHCP lease. Track egress separately from Docker readiness.
Probe at first boot, after a host network change, and after waking. Use
bounded, configurable probes that send no identifying data. Compare host
and guest results for the same destination, address family, and proxy
policy. A registry authentication challenge can establish HTTPS reachability
after certificate validation succeeds.

| Observation | Behavior in `auto` mode |
|---|---|
| Native path succeeds | Keep native NAT. |
| Host reaches the destination and native DHCP, DNS or TCP fails | Try the user-space path and confirm connectivity before reporting recovery. Publish the chosen mode and the failed stage. |
| Host and guest both fail DNS or transport | Keep the local engine usable. Report the failing stage and retry on an applicable network change. |
| Host DNS returns a policy answer, or TLS rejects a certificate | Preserve the resolver result and certificate validation. Surface the DNS, proxy or certificate error and use the configured trust path. |
| A process firewall blocks the signed helper | Report the affected helper and policy failure or timeout. Require the user's normal firewall decision. |
| User explicitly pins a mode | Keep that mode and report its connectivity status. |

Use the macOS scoped resolver for private VPN domains and split DNS. Route
helper sockets through the ordinary host network policy, with no firewall
or VPN changes. Keep the helper transport private, apply explicit host
service exposure rules, and preserve loopback-only published ports by
default. Confirm repeated failures before changing modes. Schedule a
restart-required change around active workloads and avoid repeated switching
on transient Wi-Fi, captive portals, or VPN handoffs.

### Apple silicon qualification

On macOS 27.2.1, both Marsa and a stock Ubuntu VM experienced native NAT
failures. With NordVPN disconnected and LuLu and the macOS firewall active,
a paired test booted the same Marsa image on native NAT and on a private
`VZFileHandleNetworkDeviceAttachment` backed by `gvproxy` v0.9.0. Both
received DHCP and resolved the same HTTPS destination. Native NAT timed out
while IPv4 forwarding reported off. The user-space path returned HTTP 200
with the server certificate verified. The probe used a disposable helper
with no TCP listening socket. It established the transport only. The pinned
helper's defaults map a guest address to host loopback and read
`/etc/resolv.conf` for some record types. Qualify an explicit helper
configuration with that loopback mapping controlled and all DNS requests
subject to scoped resolver policy. Validate policy outcomes with filters
enabled, including LuLu's documented limits on hostname-based filtering.
[Apple's file-handle attachment](https://developer.apple.com/documentation/virtualization/vzfilehandlenetworkdeviceattachment)
· [gvproxy v0.9.0 configuration](https://github.com/containers/gvisor-tap-vsock/blob/v0.9.0/cmd/gvproxy/config.go)
· [gvproxy v0.9.0 DNS](https://github.com/containers/gvisor-tap-vsock/blob/v0.9.0/pkg/services/dns/dns.go)
· [LuLu documentation](https://objective-see.org/products/lulu.html)

[M3-14](plan/M3-vm-and-daemon.md#m3-14-qualify-the-unprivileged-user-space-transport)
owns the transport and helper qualification.
[M3-15](plan/M3-vm-and-daemon.md#m3-15-select-the-network-mode-before-docker-starts)
owns automatic selection during boot.
[M4-10](plan/M4-docker-integration.md#m4-10-user-space-network-mode-and-automatic-selection)
owns transitions after boot and the port-forwarding policy.

The production helper in an entitled VM test reached the upstream Docker registry with NordVPN, LuLu and
the macOS firewall active. A local-only DNS-SD record resolved through the guest
and matched the macOS resolver. This Mac's independently configured lab resolver
timed out from the host during the VPN test. Positive qualification against that
resolver needs a reachable endpoint. The previous native test separately reached
HTTPS over IPv6 after router advertisement and timed out over IPv4.

Apple's published XNU source initializes host-wide IPv4 forwarding to zero.
A live zero is consistent with the default and does not identify the process or
configuration that last set it. Apple documents its native NAT attachment as the
routing and translation mechanism. Marsa uses the forwarding value only as
diagnostic context and offers user-space networking as the alternative.
[Apple XNU default](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/bsd/netinet/ip_input.c#L192-L195)
· [Apple native NAT](https://developer.apple.com/documentation/virtualization/vznatnetworkdeviceattachment).

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

The agent updates the guest hosts file and supplies Docker’s configured host gateway IPs. M4-08 applies the standard aliases to containers created through the Marsa Docker socket.

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
| HTTP and HTTPS proxy | `marsad` sends system or manual settings. The agent stages the Docker configuration before the engine starts and reports restart requirements on later changes. M4-15 integrates a controlled restart. |
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
