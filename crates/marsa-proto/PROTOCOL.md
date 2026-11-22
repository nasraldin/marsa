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

API 1.1 adds `SettingsSnapshot.restart_required` and
`EngineStatus.guest_health` without changing existing fields. API 1.0 clients
ignore both fields. Missing fields have the protobuf defaults of `false` and
unspecified health. VM stats are live observations of the active guest.
The stream starts with a sample only when the daemon has observed one.

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

### Guest message limits

Limits apply to the serialized envelope including the request ID, excluding
the four-byte prefix. Nested messages cannot be sent independently. Their
fields are additionally bounded as shown. String limits count UTF-8 bytes.

| Direction | Envelope body | Maximum | Additional bounds |
|---|---|---:|---|
| Guest → host | `Hello` | 12 KiB | Required protocol version. Image version ≤128 bytes. Up to 32 component versions (name ≤64, version ≤128 bytes) and 32 boot timings (stage ≤64 bytes). |
| Host → guest | `HelloAck` | 64 B | Required selected protocol version. |
| Host → guest | `SetTime` | 64 B | Nanoseconds <1,000,000,000. |
| Host → guest | `ConfigureNetwork` | 32 KiB | Up to 16 IP resolver addresses, 16 ASCII DNS search domains and 32 host aliases. IP literals ≤45 bytes. Domains ≤253 bytes excluding a trailing dot. Proxy URLs ≤2,048 bytes and up to 32 bypass entries of ≤255 bytes. |
| Host → guest | `InstallCertificates` | 1 MiB | Up to 128 nonempty DER blobs, each ≤16 KiB. |
| Host → guest | `Trim` | 32 B | No payload fields. |
| Host → guest | `MountShare` | 5 KiB | Tag ≤36 bytes. Absolute guest path ≤4,096 bytes, without empty, dot or parent segments. |
| Host → guest | `UnmountShare` | 128 B | Tag ≤36 bytes. |
| Host → guest | `Shutdown` | 32 B | No payload fields. |
| Host → guest | `StartEngine` | 32 B | No payload fields. |
| Host → guest | `ProbeEgress` | 32 B | Exactly four address bytes. |
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
