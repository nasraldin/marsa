# 04 Virtualization on macOS

Marsa runs one shared Linux VM on Apple silicon. Two backends implement the same [`Hypervisor` trait](03-host-daemon.md#the-hypervisor-trait).

| Backend | Built on | Role | Status |
|---|---|---|---|
| `vz` | Apple Virtualization.framework | Default backend for version 1 | Decided |
| `krun` | libkrun on Apple Hypervisor.framework | Second backend, benchmarked against `vz`; becomes default only if it wins | Decided |

## Why two backends

- **Virtualization.framework** is the official, supported, lowest-risk path. It gives near-native CPU, virtiofs file sharing, vsock and NAT networking with little code.
- Its known weak point, found during planning research, is returning freed guest memory to macOS. The fastest products on the Mac (OrbStack, Docker's own VMM) moved to custom virtual machine monitors on Hypervisor.framework to control memory and file sharing.
- **libkrun** is an open-source Rust VMM (Apache-2.0) that runs on Hypervisor.framework and links directly into a Rust process. It is the base for that faster design, but research also found a report of stock libkrun being slower than Virtualization.framework on a database workload. So it must earn its place by measurement.
- Rosetta was the main reason to be tied to Virtualization.framework. With x86 images out of scope, both backends are on equal footing.

## Architecture

```mermaid
flowchart TB
    sup["VM supervisor (marsad)"]
    tr["Hypervisor trait (marsa-vm)"]

    subgraph vzb["marsa-vm-vz"]
        vzrs["Rust backend"]
        bridge["Virtualization.framework<br/>via objc2-virtualization"]
        vzf["Virtualization.framework"]
    end

    subgraph krb["marsa-vm-krun"]
        krrs["Rust backend"]
        libkrun["libkrun"]
        hvf["Hypervisor.framework"]
    end

    guest["Linux guest"]

    sup --> tr
    tr --> vzrs --> bridge --> vzf --> guest
    tr --> krrs --> libkrun --> hvf --> guest
```

## The `vz` backend

### Virtual hardware

| Device | Framework class | Purpose |
|---|---|---|
| Boot loader | `VZLinuxBootLoader` | Boots the kernel directly with a command line; no firmware, no bootloader stage |
| CPU and memory | `VZVirtualMachineConfiguration` | CPU count and memory size from settings |
| Root disk | `VZVirtioBlockDeviceConfiguration` (read-only attachment) | The versioned guest root filesystem |
| Data disk | `VZVirtioBlockDeviceConfiguration` | Images, containers, volumes |
| Shared folders | `VZVirtioFileSystemDeviceConfiguration` | virtiofs shares for bind mounts |
| Control and Docker API | `VZVirtioSocketDeviceConfiguration` | vsock between `marsad` and the guest |
| Network | `VZVirtioNetworkDeviceConfiguration` with a NAT or file-handle attachment | See [06 Networking](06-networking.md) |
| Memory balloon | `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` | Lowering the guest's memory target |
| Entropy | `VZVirtioEntropyDeviceConfiguration` | Randomness for the guest |
| Console | `VZVirtioConsoleDeviceSerialPortConfiguration` | Kernel and init log to `console.log` |

M1-02 verified these classes in the macOS 27.2 SDK and booted the complete device configuration through both bridges on macOS 27.2.1, with a 27.0 deployment target. See the [availability table and runtime evidence](spikes/M1-02-vz-binding.md#verified-class-availability).

### Entitlement and signing

- The process that creates the VM needs the `com.apple.security.virtualization` entitlement. `marsad` carries it.
- An unsigned or ad hoc signed development build must still include the entitlement, or VM creation fails.
- See [12 Build, release and update](12-build-release-update.md).

### Threading

The VZ backend allocates a private GCD serial queue per VM and supplies it to
`VZVirtualMachine.initWithConfiguration:queue:`. It checks host files on
Tokio's blocking pool before submitting configuration. All Foundation and VZ
allocation, validation, device setup, lifecycle calls and owned releases run
on that queue. Jobs assert their queue and use an autorelease pool.

The native VM, retained delegate and active socket connections live in a
private slot. Delegate and lifecycle completion callbacks assert the queue and
enqueue a job to avoid reentering the slot. Cancelling a Rust start leaves
the native completion in charge of the state transition. Dropping the last
VM handle queues teardown and retains the VM through in-flight start and
forced-stop calls. The socket connect call releases
its slot borrow before calling VZ so the completion can retain the connection
even when invoked inline. Tokio receives a close-on-exec descriptor duplicate,
which it registers in nonblocking mode, and a Rust lease that releases the
native connection on the VM queue. The lease also handles cancelled connect
futures. The VM releases remaining connections on stop or teardown. Apple's
[descriptor contract](https://developer.apple.com/documentation/virtualization/vzvirtiosocketconnection/filedescriptor)
assigns ownership of the original descriptor to the connection.

The backend attaches both raw images with automatic caching and full
synchronization, as selected in [M1-03](spikes/M1-03-disk-reclaim.md). The root
attachment is read-only. The virtio console writes directly to an append-only
descriptor opened before the VM starts. Marsa opens a private user-owned
regular file and checks for symbolic links and inode aliases.

With the pinned Linux 7.3.5 kernel, virtio-console discards the kernel's
initial replay while registering `hvc0`. The messages remain in the kernel
ring. On entering PID 1, `marsa-init` reads a bounded, non-destructive snapshot
and emits the messages before `hvc0` registration in a marked section of
`console.log`. If the kernel stops before reaching PID 1, the file contains
the output the guest managed to send over the live console.
[Linux kernel mailing list investigation](https://lkml.iu.edu/hypermail/linux/kernel/2610.3/17591.html).

The [entitled system tests](../crates/marsa-vm-vz/README.md) boot the packaged
guest on Apple silicon. They check native lifecycle events, a versioned agent
handshake, root device and filesystem write rejection, and ordered console
output from the kernel through Docker API readiness.
[Apple's initializer contract](https://developer.apple.com/documentation/virtualization/vzvirtualmachine/init(configuration:queue:)).

### Calling the framework from Rust

The production backend uses generated `objc2-virtualization` bindings as approved in [D-024](15-decisions.md#d-024-use-direct-rust-bindings-for-virtualizationframework). Both approaches passed the [M1-02 boot, vsock and lifecycle trial](spikes/M1-02-vz-binding.md).

| Option | For | Against |
|---|---|---|
| Rust bindings (`objc2` family) | One language, no second build system, no FFI layer to maintain | Bindings must cover every class Marsa needs; verify completeness |
| Small Swift shim with a C ABI | Uses the framework in its native language; delegate callbacks are simple | Adds Swift to the build and a hand-written FFI boundary |

Keep the generated bindings and callback lifetime handling within the macOS backend.

### Boot sequence

```mermaid
sequenceDiagram
    autonumber
    participant S as VM supervisor
    participant B as vz backend
    participant K as Linux kernel
    participant I as marsa-init
    participant A as marsa-agent
    participant E as dockerd

    S->>B: create(VmSpec) then start()
    B->>K: Boot kernel directly with cmdline
    K->>I: Run marsa-init as PID 1
    I->>I: Mount root read-only, mount data disk, set up cgroup v2
    I->>I: Mount virtiofs shares, bring up network
    I->>A: Start marsa-agent
    A->>S: Handshake over vsock (versions, boot timings)
    S->>A: Push time, DNS, proxy, CA certificates
    I->>E: Start containerd then dockerd
    A->>S: Engine healthy
    S->>S: State = Ready
```

Boot-time budget (**Proposed**, measured by the agent handshake timings): kernel under 300 ms, init to agent handshake under 300 ms, dockerd healthy under 1.5 s.

## The `krun` backend

- Links libkrun into `marsad` (or a small helper process if isolation of the VMM proves necessary).
- Uses the same guest kernel configuration where possible. **Verify** whether libkrun requires its own bundled kernel (libkrunfw) or can boot Marsa's kernel; this affects the guest image build.
- Maps the same `VmSpec`: virtio block for disks, virtiofs for shares, vsock for control, virtio-net for networking.
- Needs the `com.apple.security.hypervisor` entitlement.
- **Verify** the current libkrun release, its macOS requirements and its C API before starting; planning research found version 1.19.x with macOS 14 or newer on arm64.

## Choosing the default: the benchmark gate

`vz` stays the default unless `krun` passes this gate (task group M11).

| Measure | `krun` must be |
|---|---|
| Bind-mount read and write throughput | At least equal to `vz` |
| Cold-cache directory walk of a large shared tree | Faster than `vz` |
| Database import on the data disk | Within 5% of `vz` |
| Idle memory after a 2 GiB workload ends | Lower than `vz` |
| Cold start to first container | Within 10% of `vz` |
| Stability | 72-hour soak test with sleep and wake cycles, no failures |

## Memory management

**Proposed**, with every mechanism to be measured:

1. **Right-size at start.** Default memory is a fraction of host RAM, capped, and changeable in settings.
2. **Balloon on idle.** When containers stop, lower the guest memory target through the balloon device. **Verify** how much memory Virtualization.framework actually returns to macOS; planning research found this is its weak point.
3. **Stop when idle.** With no running containers for `idle_timeout`, stop the VM. This always returns all memory and is the dependable mechanism.
4. **Fast restart.** Because boot is fast, stopping when idle is nearly invisible to the user.

## Failure handling

| Failure | Detection | Response |
|---|---|---|
| VM exits unexpectedly | Backend state event | Move to `Failed`, capture console tail, retry with backoff |
| Agent handshake timeout | Supervisor timer | Stop the VM, report `Failed` with the boot stage reached |
| dockerd never healthy | Agent health report | Report the engine log tail; keep the VM up for diagnosis if the user asks |
| Entitlement missing | VM creation error | Fail fast with a clear message; this is a packaging bug |

## Not used

| Technology | Why not |
|---|---|
| Apple's Containerization framework and `container` tool | One VM per container and not the Docker API; version 0.1.0 at planning time |
| Lima | A separate Go tool driven by SSH; useful as a bootstrap for early GUI work only, never shipped |
| QEMU | Emulation and a heavy stack; against the native-only principle |
| Rosetta | x86 images are out of scope |
