# Marsa wire protocols

## Build the Rust bindings

Cargo generates the bindings in its build output from the `proto/` directory.
The host feature is enabled by default. The guest agent disables it so its
static Linux binary only uses the guest messages and codec.

Install the compiler for the **build host** from the upstream v36.2 release.
Verify the archive's SHA-256 digest, unpack it, then add its `bin` directory
to `PATH` or set `PROTOC` to the absolute path of `bin/protoc`.
Cross-compiling the guest from macOS uses the macOS compiler.
[Protocol Buffers v36.2 release](https://github.com/protocolbuffers/protobuf/releases/tag/v36.2)

| Build host | Archive | SHA-256 |
|---|---|---|
| Apple silicon macOS | `protoc-36.2-osx-aarch_64.zip` | `9cd98a532c5c5e0c4161314de0225de27e4c8a323917b6ea7b1b714d3ae23466` |
| Linux amd64 | `protoc-36.2-linux-x86_64.zip` | `121f6c7afe1d4d0e3ea6aab9432038599250134cbf4474cb1167d2c7decd4278` |
| Linux arm64 | `protoc-36.2-linux-aarch_64.zip` | `8b8f18bd2b30346efbc698dd5a73dd7c805f3ef8380f6dfc95c768f3f1852f6a` |

```sh
cargo build --locked -p marsa-proto
rustup target add aarch64-unknown-linux-musl
cargo build --locked -p marsa-proto --no-default-features --target aarch64-unknown-linux-musl
```

## Host API

The host serves `marsa.v1` as gRPC over the local Unix socket.
`Meta.Version` returns the API major and minor numbers. Clients require the same
major and only use features available in the daemon's minor revision. Within a
major version, changes are additive and retired field numbers and names must be
reserved. The daemon enforces request validation and authentication at the
service boundary. Mutations carry a UUID for daemon-side deduplication.
Engine lifecycle calls and settings replacements each retain a bounded,
process-local history, evicting completed requests before admitting new ones.
Retry an interrupted request with the same UUID and payload. Reusing a cached
UUID with a different operation or payload fails with `ALREADY_EXISTS`.

Host API minor 3 adds settings reset. Send `PrepareSettingsReset`, obtain user
confirmation, then send `ResetSettings` with that token and a separate request
UUID. The token is scoped to the current configuration state.
The same minor adds the explicit `DockerDiscovery.EnableRegistryKeychain`
mutation. The RPC accepts no credentials and returns only whether it added
the setting.

Host API minor 4 adds factory reset. Use the separate `PrepareFactoryReset`
and `ConfirmFactoryReset` messages. The confirmation binds to private storage
identities and expires after 60 seconds. A committed factory reset stops the
guest, restores settings defaults, clears grants and installs a fresh sparse
data disk. An interrupted transaction is resolved before the daemon serves
either socket.

Host API minor 6 adds the separate owner Agents service and capability-scoped
AgentOps endpoint. They negotiate through the daemon and keep owner approval
authentication separate from delegated agent sessions.

Host API minor 12 adds the `ui.setup_complete` owner setting and optional
`PrunePreviewResponse.observed_usage` with measured Docker category totals.
The latter is read-only and does not authorize cleanup.

Host API minor 13 adds the owner-only `Stats.DockerOverview` snapshot.
The caller selects a fixed scope of Docker reads and receives bounded Engine
JSON. The request requires an already-running VM and does not activate it.

Host API minor 14 extends `Stats.DockerOverview` with `prefer_cache` and
`observed_at_unix_millis`. The daemon keeps last-known inventory sections on
disk and may return them without contacting Docker. Container stats remain
live-only. See D-034.

Host API minor 15 adds `DaemonEvent.docker_inventory_changed`. The daemon
coalesces changes from the already-running engine and sends an invalidation
on the same ordered stream as lifecycle events. It contains no Docker event
payload. Fetch fresh container, image, volume or network sections using
`Stats.DockerOverview`. Each of those live collection responses can carry
up to 8 MiB of JSON. Cached sections retain their 2 MiB limit.

Host API minor 16 adds a finite recent Docker activity scope to
`Stats.DockerOverview`. Activity accepts a 1, 6 or 24 hour lookback and
defaults to 24 hours. The daemon contacts only an already-running guest
and bounds the newline-delimited Engine response. It does not persist
activity in the inventory cache.

Host API minor 18 adds the owner-only `VolumeFiles.Read` stream. A client
can probe the running built-in guest or send `select` with a volume name
and Docker creation timestamp. The guest establishes the pinned volume and
confirms it before the client accepts a path. The client submits the path
and format preference in `prepare`, then sends `start` after choosing the
destination. The client requires successful completion and stream termination
before publishing its staged destination.

Host API minor 20 adds credential-free effective HTTP and HTTPS proxy endpoints
and the bypass list to the owner-only `PrepareProxyActivation` preview. Approval
is bound to the resolved policy as well as the settings and lifecycle revisions.

Host API minor 19 implements `Network.ListForwardedPorts`. Its `ports` list
contains active host listeners. Each entry in `failures` describes a requested
binding by address, port, transport and container ID, with a diagnostic and an
optional local process name and PID. The events snapshot and stream expose the
same inventory. Changing `network.allow_lan` applies to current publications.

`Disk.Reclaim` requests discard of unused blocks from the current ready
guest's data disk. It returns the guest's submitted discard bytes and the
observed decrease in allocated bytes on the same host image. A stopped
VM is not started for reclaim. The daemon admits one trim at a time and
replays the result when the caller retries the same request UUID.

`Events.Subscribe` emits a snapshot before live updates on each connection.
The daemon takes that snapshot at an event cursor and then emits events after
that cursor in order. Sequence numbers are scoped to the stream ID of the
daemon process. If a subscriber falls behind the bounded buffer, the daemon
ends the stream with `RESOURCE_EXHAUSTED`. Clients replace their view with the
new snapshot on reconnect.
Host minor 2 introduces `DockerDiscovery.GetEndpoint` and
`DockerDiscovery.EnsureContext`. Registration is explicit, and the request has
no shell command, executable path, or context selection fields.

The API design and service table live in the daemon design.
[Host typed API](../../docs/03-host-daemon.md#typed-api)

Settings messages carry a revision and the canonical TOML document. An update
must match the current revision. Read it again after reconnecting to a
different daemon process. The daemon publishes a new revision only after the
replacement has committed. `Settings.Watch` starts with the current settings.

Host API minor 10 adds optional ConfigurationIssue details to rejected
settings updates and share operations. The status code and legacy message
keep their existing meanings. The details encode a reason and, where
relevant, a setting key, document position, size limit, or schema version.
They carry no configuration values, credentials, or paths. Clients can
localize recognized reasons and use the legacy message with older daemons.

Host API minor 7 implements `Network.SetMode` for the explicit auto, NAT and
user-space enum values.
It persists a single-field settings edit. A successful response reports the
mode saved by that request, plus the independently observed VM attachment
and addresses when available. Active fields remain empty when the observer
has no guest. Replay the same UUID and mode for the original mutation
response, or call `Network.Status` for a fresh observation. Both replies can
carry `NetworkStatus.restart_pending`. It compares the configured value in
the response with the mode captured when the current VM launched and is
false when no VM is running. It is absent when the boot policy is unknown
or still being applied, including replies from older daemons. An automatic
fallback changes the active attachment without making a restart pending.
Engine restart is a separate action.


API 1.1 adds `SettingsSnapshot.restart_required` and
`EngineStatus.guest_health` without changing existing fields. API 1.0 clients
ignore both fields. Missing fields have the protobuf defaults of `false` and
unspecified health. VM stats are live observations of the active guest.
The stream starts with a sample only when the daemon has observed one.

API 1.5 adds `Network.PrepareProxyActivation` and `Network.ActivateProxy`.
Clients read the current settings revision, request a preview, and show every
reported potential interruption before asking the owner to approve. Apply
only after approval using the preview token, the same settings revision and a
fresh request UUID. The token expires after 90 seconds and is single-use.
Replay a disconnected request with the same UUID and fields to obtain the
original result. A changed VM lifecycle or settings revision invalidates the
preview. Result enums report failed stop, start or persistence and whether the
prior policy or engine was recovered. Treat approval tokens and proxy URLs as
sensitive.

Agent requests identify an
allowlisted operation and use that operation's existing JSON Schema for its
arguments. The daemon authenticates the client, validates those arguments, and
applies permission and audit policy before sending any request to the engine.
[Agent policy](../../docs/10-mcp-server.md#permission-tiers)

## Guest control channel

The guest listens on AF_VSOCK port 5000 and accepts the host CID. Port 5001 is a separate unframed, host-CID-only byte stream to the guest Docker socket. Send `HelloAck` and an initial `SetTime` before expecting the Docker engine to become ready. The agent normally releases PID 1 to start Docker after setting the clock.

The host opens the control vsock connection to the guest. Each record begins
with an unsigned four-byte big-endian integer containing the protobuf payload
length. Read that header and call `frame_length` before reserving a buffer or
reading the body. Read exactly that many bytes, then call `decode_host` or
`decode_guest` for the expected direction. The global payload limit is 1 MiB.
The separate Docker API bridge uses the Docker byte stream directly.

The guest bind-authorization channel uses a separate host-initiated vsock
stream on port 5003. Each length-prefixed protobuf payload is at most 64 KiB.
Queries carry only the explicit absolute bind sources and a request ID.
Version 1.0 requires matching sequential IDs and a valid allow or deny response.
The guest denies these binds if the policy channel cannot answer. Boot-mounted
virtiofs roots remain the host isolation boundary.

The guest volume read channel uses dedicated host-initiated vsock port 5004.
It uses the same four-byte length prefix as the control channel, with a bounded
payload and at most 64 KiB of file bytes per frame. One selection is permitted
per connection. The guest holds the same selection across preparation and
transfer and sends a final success after physical and engine identity
checks. Older guests lack this capability and cannot serve a download.

1. After connecting, the guest sends `Hello` with request ID zero. The image
   version, component versions and boot timings describe the running image.
2. The host requires the same major and selects the smaller minor with
   `negotiate`. Incompatible majors end the connection. The host returns
   `HelloAck` with the selected version and request ID zero.
3. The guest checks that the selected major matches its advertised major and
   that the selected minor is no greater than the one it advertised. The host
   waits for its handshake before sending commands. The guest waits for a valid
   acknowledgement before applying commands or sending events.
4. Each command has a nonzero request ID, unique among pending requests on
   that connection. A result echoes its command's ID. Unsolicited health, port
   and stats events use request ID zero. The recipient rejects results with
   no corresponding pending command or with the wrong result type. A successful
   `Trim` returns `TrimResult`. A failed `Trim` returns a non-OK `CommandResult`.

A reconnect starts a new handshake and clears pending requests. A peer may
send new message variants only when the negotiated minor supports them.
Additive fields for existing variants use new field numbers and must preserve
existing behavior when omitted. Close the connection on malformed, oversized
or out-of-sequence input. Transport owners apply timeouts to incomplete frames and handshakes.
Wire validation checks shape. The agent still checks mount allowlists,
certificate validity and operating system constraints before applying changes.
Treat peer diagnostics and proxy URLs as potentially sensitive.

### Deferred engine start (guest protocol 1.1)

A host that negotiates minor 1 or later can set `HelloAck.defer_engine_start`.
The guest then acknowledges `SetTime` without releasing Docker. After the clock
is set, the host may send `ProbeEgress` on the deferred session. It supplies the
four network-order bytes of an IPv4 address selected for
`https://registry-1.docker.io/v2/`. The guest retains the hostname for DNS, TLS
and HTTP checks. The probe returns a correlated `EgressResult` with a diagnostic
stage. A probe outside the permitted state or one that exceeds its control
deadline returns `POLICY`. Neither is evidence of blocked native egress.

After the host selects the attachment, `StartEngine` releases PID 1 and
returns `CommandResult`. It requires a successful `SetTime` and is idempotent.
`Shutdown` remains available before Docker is released. Hosts that negotiate
1.0 or omit the deferral flag retain the original `SetTime` startup behavior.
Once Docker has been released, reconnecting cannot make that guest eligible for
another deferred probe.

### Published port relay

AF_VSOCK port 5005 accepts connections from the host CID. Each stream carries
one TCP connection or one UDP peer flow. The host sends five bytes: version
1, IP protocol number (6 or 17), family (4 or 6), and the guest's published
port as a nonzero big-endian u16. The guest connects to its own loopback
address in that family. A zero reply accepts the stream. A nonzero reply
or a closed connection fails the request. TCP then carries bytes in both
directions. Each UDP datagram has a big-endian u16 length followed by its
payload, including a zero-length payload. The Mac obtains the publication
from the active guest control session and applies the owner's binding policy.

### Native NAT resolver policy (guest protocol 1.2)

A negotiated minor of 2 supports the optional `ConfigureNetwork.native_dns`
field. The host sends the default servers, search suffixes and supplemental
domain routes before engine release and again after host network changes.
An empty route server list keeps the suffix local-only. An explicit DNS
override uses only the configured servers. This update preserves the native
Docker gateway and does not restart the engine. Minor 1 accepts only a flat
resolver list. When a host policy requires split DNS or has no usable servers,
the host provisions a local DNS sink so the local engine can still run.
Minor 0 retains DHCP when following the host and refuses explicit overrides
that it cannot apply before Docker starts. Live native refresh requires 1.2.

### Live egress checks (guest protocol 1.3)

After clock synchronization and engine release, a host that negotiated minor 3
can send `ProbeLiveEgress` to recheck the active attachment. It uses the same
registry, address validation, deadline and correlated result as the boot probe.
Older guests keep serving local workloads when the host network changes.
The live command does not release or restart Docker.

### Registry configuration (guest protocol 1.4)

The host sends `ConfigureNetwork.registries` before engine startup when the
owner configured registry mirrors or insecure registries. A peer below minor
4 cannot apply those overrides. The host refuses that boot. A DNS-only refresh
omits the field and preserves the engine configuration. The guest validates
both lists before writing Docker's configuration.

### Guest message limits

Limits apply to the serialized envelope including the request ID, excluding
the four-byte prefix. Nested messages cannot be sent independently. Their
fields are additionally bounded as shown. String limits count UTF-8 bytes.

| Direction | Envelope body | Maximum | Additional bounds |
|---|---|---:|---|
| Guest → host | `Hello` | 12 KiB | Required protocol version. Image version ≤128 bytes. Up to 32 component versions (name ≤64, version ≤128 bytes) and 32 boot timings (stage ≤64 bytes). |
| Host → guest | `HelloAck` | 64 B | Required selected protocol version. |
| Host → guest | `SetTime` | 64 B | Nanoseconds <1,000,000,000. |
| Host → guest | `ConfigureNetwork` | 32 KiB | Up to 16 IP resolver addresses, 16 ASCII DNS search domains, 32 host aliases and 64 native DNS suffix routes of up to 16 IP servers each. IP literals ≤45 bytes. Domains ≤253 bytes excluding a trailing dot. Proxy URLs ≤2,048 bytes and up to 32 bypass entries of ≤255 bytes. Up to 16 registry mirror URLs of ≤1,024 bytes and 32 insecure registry entries of ≤260 bytes. |
| Host → guest | `InstallCertificates` | 1 MiB | Up to 128 nonempty DER blobs, each ≤16 KiB. |
| Host → guest | `Trim` | 32 B | No payload fields. |
| Host → guest | `MountShare` | 5 KiB | Tag ≤36 bytes. Absolute guest path ≤4,096 bytes, without empty, dot or parent segments. |
| Host → guest | `UnmountShare` | 128 B | Tag ≤36 bytes. |
| Host → guest | `Shutdown` | 32 B | No payload fields. |
| Host → guest | `StartEngine` | 32 B | No payload fields. |
| Host → guest | `ProbeEgress`, `ProbeLiveEgress` | 32 B | Exactly four address bytes. |
| Guest → host | `PortEvent` | 256 B | Known action and TCP or UDP transport, 64-digit hex container ID, IP literal ≤45 bytes, ports 1–65,535. |
| Guest → host | `EngineHealth` | 3 KiB | Known health value and detail ≤2,048 bytes. |
| Guest → host | `VmStats` | 128 B | Memory and disk usage cannot exceed their corresponding totals. |
| Guest → host | `TrimResult` | 64 B | Trimmed and used byte counts. |
| Guest → host | `CommandResult` | 3 KiB | Known status and detail ≤2,048 bytes. |
| Guest → host | `EgressResult` | 32 B | A known, nonzero diagnostic stage. |

## Guest network facts

AF_VSOCK port 5002 accepts the host CID. The guest sends one nine-byte
record and closes the connection. The record contains version byte `1`,
four IPv4 address bytes and four default gateway bytes in network byte
order. Four zero bytes represent an absent address or gateway. If the
address is absent, the gateway must also be absent. The host validates the
record before retaining the facts, and abandons a connection that does
not respond within its deadline. The guest reads the configured upstream
interface and its default route from the kernel. The record describes
guest state and is independent of the guest control session.

## Bind authorization

The host initiates a dedicated AF_VSOCK connection to guest port 5003.
Only the host CID can enter the listener. The guest sends
`BindAuthorizationRequest` and the host replies with
`BindAuthorizationResponse`. Each protobuf payload has a four-byte
big-endian length prefix, with a limit of 64 KiB. This channel declares
protocol 1.0 in each frame independently of the guest control session.

One request is outstanding at a time. The guest starts request IDs at one
after each connection and increments by one for each query. A response must
echo the outstanding ID. Unexpected IDs, versions, incomplete frames or a
deadline close the stream and provide no authorization.

Queries carry only one to 64 absolute UTF-8 bind source strings of at most
4096 bytes each. A request supplies candidate paths, not approvals or an
attachment inventory. The host validates every source against the grants
and attachments for that VM. A denial carries an actionable message of at
most 1024 bytes. An allow has an empty message. Source and message strings
exclude control characters. Neither the Docker body nor any headers cross
this channel.
