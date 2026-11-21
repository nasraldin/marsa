# Marsa user-space network helper

The macOS arm64 helper uses the Ethernet switch, DHCP server, and vendored
gVisor TCP/IP stack from gvisor-tap-vsock v0.9.0. The helper receives Ethernet
datagrams on an inherited connected Unix socketpair endpoint in standard input.
It takes no arguments, runs unprivileged in the foreground, and writes one
`READY` line after the guest stack and DNS endpoints are started. Standard error
is reserved for diagnostics. There are no host listening sockets, socket files,
port forwards, or SSH/control APIs. The VZ network controller owns the process
lifetime and the other socket endpoint.

The isolated IPv4 network is `192.168.127.0/24`, the virtual gateway and guest
DNS are at `192.168.127.1`, and DHCP assigns `192.168.127.2` to
`5a:94:ef:e4:0c:ee`. The helper uses ordinary host sockets for TCP and UDP
egress. It rejects host interface addresses, IPv4 loopback, link-local and
reserved destinations, known cloud metadata addresses, and the virtual subnet.
The guest's only plaintext DNS path is its virtual gateway. IPv6 transport
and inbound port forwarding are outside this helper.

DNS questions go through the macOS system resolver using
`DNSServiceQueryRecord`. The system chooses the scoped resolver for the queried
name. The helper has no public DNS fallback and does not read
`/etc/resolv.conf`. It accepts one Internet-class question, returns bounded DNS
records including non-address types when the system answers, and fails
explicitly on unsupported requests, oversize records or timeout. The guest
chooses any search suffixes it applies to relative names. The helper does not
claim to preserve DNSSEC validation flags from a synthesized reply.
[Apple system DNS guidance](https://developer.apple.com/documentation/technotes/tn3151-choosing-the-right-networking-api).

## Build and check

The source is the signed upstream tag `v0.9.0` at commit
`6dbb5d4fa0033356c72a201346f8350594229868`, pinned to the SHA-256 digest of the
commit archive `03cf1d57ae8c9fccd9146b55616ed1da11f5cdd9de9d39740d3ce4b14f6abb3a`.
The archive contains the complete vendor tree. The script verifies the digest
before unpacking and adds this directory's source as a new command. The Go
compiler runs with module downloads, toolchain downloads and build host paths
disabled.
[Upstream release](https://github.com/containers/gvisor-tap-vsock/releases/tag/v0.9.0).

Use Go 1.27.4 for macOS arm64 and Xcode's macOS SDK. The reference build used
SDK 27.2, Apple clang 21.0.0 (`clang-2100.4.1.1`) and deployment target 27.0.
The Go linker produced byte-identical binaries in two independent source
directories, with SHA-256
`f7d099c984bc96be077b3d5827a4fb17326cf511cac5b5b8c57e10185fe9e5e4`. The SHA-256 of Go's macOS arm64 archive is
`91a44a2b8a716600ad94a4f143e156116620db8828086a6e3602291be9bd0bb4`.
[Go downloads and checksums](https://go.dev/dl/).

```sh
MARSA_NETWORK_HELPER_TEST=1 bash network-helper/build.sh target/network-helper/marsa-net-helper
python3 network-helper/check.py target/network-helper/marsa-net-helper
```

For a cached or offline build, set `MARSA_NETWORK_HELPER_ARCHIVE` to the pinned
archive. A fixture can query an existing host-scoped name using `--scoped-name`
and optionally `--record-type`. For example, an SOA question exercises the
non-address record path without changing host resolver settings.

```sh
python3 network-helper/check.py target/network-helper/marsa-net-helper \
  --scoped-name example.com --record-type 6
go version -m target/network-helper/marsa-net-helper
govulncheck -mode=binary -show verbose target/network-helper/marsa-net-helper
```

The fixture checks readiness and shutdown, Ethernet framing, DHCP, rejected
loopback access, a local-only DNS name, and explicit refusal for an unsupported
query. The Go tests cover blocked metadata and local addresses and one scoped
resolver call per allowed DNS request.

## Licence and advisory evidence

The linked module versions below come from `go version -m`. The upstream
archive provides the corresponding licence files under `vendor/` and the
gvisor-tap-vsock licence at its root. Preserve those licences and copyright
notices with distributions of the helper. Go's distribution includes its
own BSD-3-Clause licence and patent grant.

| Component | Version | Licence |
|---|---|---|
| gvisor-tap-vsock | `v0.9.0` | Apache-2.0 |
| gVisor | `v0.0.0-20260413194555-9680d69bf798` | Apache-2.0 and BSD-3-Clause as marked in source |
| `github.com/google/btree` | `v1.1.2` | Apache-2.0 |
| `github.com/google/gopacket` | `v1.1.19` | BSD-3-Clause |
| `github.com/insomniacslk/dhcp` | `v0.0.0-20240710054256-ddd8a41251c9` | BSD-3-Clause |
| `github.com/miekg/dns` | `v1.1.73` | BSD-3-Clause |
| `github.com/pierrec/lz4/v4` | `v4.1.14` | BSD-3-Clause |
| `github.com/sirupsen/logrus` | `v1.10.2` | MIT |
| `github.com/u-root/uio` | `v0.0.0-20240224005618-d2acac8f3701` | BSD-3-Clause |
| `golang.org/x/exp` | `v0.0.0-20231110203233-9a3e6036ecaa` | BSD-3-Clause |
| `golang.org/x/sys` | `v0.48.0` | BSD-3-Clause |
| `golang.org/x/time` | `v0.12.0` | BSD-3-Clause |

The November 22, 2026 binary scan with govulncheck v1.10.0 found no affected
symbols or packages. The linked DHCP module has one module-level advisory
for its raw DHCP **client**. The helper uses the server and does not link
the affected `dhcpv4/nclient4` package. Checked with `go tool nm`.
[GO-2026-6237](https://pkg.go.dev/vuln/GO-2026-6237).
