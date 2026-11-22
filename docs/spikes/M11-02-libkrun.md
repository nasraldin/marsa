# M11-02 libkrun qualification

Recommend stopping the version 1 libkrun backend here. The latest stable release booted Marsa's unmodified guest, but its documented virtiofs security model requires a separate host isolation boundary to meet Marsa's explicit share policy. Keep Virtualization.framework as the default under [D-008](../15-decisions.md#d-008-virtualizationframework-default-libkrun-benchmarked). Reconsider libkrun once there is a supported, reviewed macOS design that confines the VMM to the approved directories and devices.

## Version and boot result

- Verified upstream libkrun 1.19.8, commit `c4d283c233bc0ae1993c289ec2a1b4da2242f1f8`, on 2026-11-22. Its library is Apache-2.0. The generic arm64 macOS variant requires macOS 14 or newer. The signed probe used macOS 27.2.1 and deployment target 27.0. [Release](https://github.com/libkrun/libkrun/releases/tag/v1.19.8), [macOS requirements and security model](https://github.com/libkrun/libkrun/blob/c4d283c233bc0ae1993c289ec2a1b4da2242f1f8/README.md).
- Built the release's C API with block and network devices, without its optional init blob. The probe verifies Marsa's kernel and root-image digests before launch. It booted Linux 7.3.5 directly without a separate firmware library. Linux mounted the EROFS root read-only and then initialized and mounted a fresh 1 GiB ext4 data image. The guest sent a protobuf `Hello` over vsock. Its image version was 0.1.0. [Pinned header](https://github.com/libkrun/libkrun/blob/c4d283c233bc0ae1993c289ec2a1b4da2242f1f8/include/libkrun.h).
- The VM recognized a read-only virtiofs tag for an empty fixture directory. The virtio-net attachment used a disconnected Unix datagram pair and TSI was disabled. This checked devices and the guest handshake. Docker conformance, throughput, sustained memory release and production network egress were outside the probe.

## Mapping to VmSpec

| Field | Qualification and integration cost |
|---|---|
| `cpus` | Two booted successfully. The C API takes an unsigned 8-bit value. Validate supported limits before converting. |
| `memory_bytes` | The guest saw 1 GiB. Require MiB alignment and validate the unsigned 32-bit MiB limit. Free-page reporting observed on boot does not measure memory returned to the host. |
| `kernel` | Booted the unmodified arm64 Linux `Image` with `KRUN_KERNEL_FORMAT_RAW`. The probe did not need libkrunfw. |
| `initramfs` | The pinned API accepts an optional path. The shipped image boots without it. Further qualification is needed before using a nonempty initramfs. |
| `cmdline` | Supplied Marsa's root, init, console, network and fixture mount options through `krun_set_kernel`. |
| `root_disk` | The same raw EROFS image mounted read-only on `/dev/vda`. |
| `data_disk` | The same raw-image format mounted on `/dev/vdb`. Both attachments selected full synchronization explicitly. Backend switching, discard and interrupted writes need independent tests. |
| `shares` | The fixture tag reached Linux. Read-only mode controls writes. Upstream requires additional host isolation to hide everything outside a share. This is a production blocker. |
| `network` | An explicitly supplied virtio-net socket can connect to an external user-space proxy. No native Apple NAT choice is provided by this C API. A future adapter would need to qualify Marsa's helper transport and report only the modes it supports. |
| `console_log` | `krun_set_console_output` captured Linux and Marsa init output. A product helper would own the log descriptor and rotate the log. |

The vsock API binds a host Unix stream to a specific guest port and returned the expected validated greeting on port 5000. The probe disables implicit vsock and explicitly requests zero TSI features. A production backend also needs to verify lifecycle, socket teardown and cancellation, disk integrity, and isolation before offering external workloads. The stable C API runs `krun_start_enter` in a blocking VMM event loop, so isolating the VMM in a separate process affects supervision and stop/restart behavior. No product dependency or entitlement was added.
