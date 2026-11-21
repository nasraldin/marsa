# M1-02 Virtualization.framework binding spike

**Recommendation for G2.** Use `objc2-virtualization` for `marsa-vm-vz`.
The direct Rust binding configured and booted the full planned virtual device
list and delivered the same callbacks as the Swift bridge. The owner approved this recommendation in [D-024](../15-decisions.md#d-024-use-direct-rust-bindings-for-virtualizationframework).

## Test environment

- Tested 2026-11-21 on a physical Apple silicon Mac, macOS 27.2.1, Xcode 27.2 build 27B52, macOS 27.2
  SDK, Swift 6.4.2, Rust 1.100.0. Both host builds use a macOS 27.0
  deployment target. The host ran as the logged-in standard user.
- Alpine 3.24.4 ARM64 virt ISO, SHA-256
  `cf88d8ba67389d20d9ed307366b52e487c15d7d3ee3233b62b95f709daa1d430`.
  The stock 6.18.60-0-virt EFI zboot payload decompresses to an ARM64 Linux
  Image with SHA-256
  `a2cecb9b6fdbd83e87bd43c8040de67300d5b8dde1fc50c34714faa272888f17`.
  The harness uses the release initramfs plus a small CPIO appendix and an
  unmodified matching module image. [Alpine 3.24.4 release files](https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/) · [Linux 6.18.60 zboot header](https://github.com/gregkh/linux/blob/v6.18.60/drivers/firmware/efi/libstub/zboot-header.S) · [ARM64 Image format](https://docs.kernel.org/6.18/arch/arm64/booting.html)
- Exact crate pins are `objc2-virtualization 0.3.2`, `objc2-foundation 0.3.2`,
  `objc2 0.6.5`, `block2 0.6.2`, and `vsock 0.5.5` for the guest fixture.
  [Rust bindings API](https://docs.rs/objc2-virtualization/0.3.2/objc2_virtualization/) · [Swift stable C ABI exports](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0495-cdecl.md)

## Results

| Condition | Direct Rust | Swift C bridge |
|---|---|---|
| Ad hoc signature with `com.apple.security.virtualization` | Verified | Verified |
| Framework configuration validates with all planned device classes | Pass | Pass |
| Alpine kernel boots and interactive serial shell prompt appears | Pass | Pass |
| Guest observes a read-only module disk and a writable data disk | Pass | Pass |
| Guest mounts virtiofs and reads the expected fixture file | Pass | Pass |
| Host sends a random 16-byte challenge over vsock and sees the exact bytes returned | Pass | Pass |
| Rust receives VZ start completion and guest poweroff delegate callback | Pass | Pass |

The guest printed `MARSA_DEVICES_READY`, `MARSA_VSOCK_READY` and
`MARSA_SHELL#` before the runner issued a command and observed its exit status.
The vsock check used the independent connection returned by
`VZVirtioSocketDevice`, and both bridges kept that connection alive while Rust
used a duplicated descriptor for timed I/O.

## Comparison

The direct bridge keeps VM lifetime, the Objective-C delegate and completion
blocks in Rust. The build links the system framework through the published
generated bindings. This route needs explicit `unsafe` at the documented
Objective-C boundary, a retained delegate, and a block wrapper per asynchronous
call. The verified classes and methods are available without custom selectors
beyond the standard delegate implementation.

The Swift bridge calls the same framework APIs with Swift's typed results and
delegate methods. Its build additionally invokes Swift, links a dylib, and
tracks ownership of the C callback context and retained Swift session. The
comparison still uses unsafe Rust at the C boundary. The direct bridge uses
263 source lines. The Swift route uses 325 lines across its Swift code, Rust
adapter, and build script. These counts include comments and exclude the
shared runner. Both routes use the published APIs for all exercised devices.

Both routes use the main serial queue for this command-line harness. The
production backend still owns its dedicated serial queue as planned. VZ API
calls, delegate notifications, and completion handlers run on the VM queue.
Socket bytes are exchanged off that queue. [Apple queue contract](https://developer.apple.com/documentation/virtualization/vzvirtualmachine/init%28configuration%3Aqueue%3A%29) · [VZ socket file descriptor](https://developer.apple.com/documentation/virtualization/vzvirtiosocketconnection/filedescriptor)

## Verified class availability

The macOS 27.2 SDK headers give the following minimum availability. The
runtime trial used the listed classes on macOS 27.2.1. The deployment baseline
remains macOS 27.0.

| Function | Verified classes | Available since |
|---|---|---|
| Direct Linux boot | `VZLinuxBootLoader` | 11.0 |
| Configuration and lifecycle | `VZVirtualMachineConfiguration`, `VZVirtualMachine`, `VZVirtualMachineDelegate` | 11.0 |
| Raw block images | `VZDiskImageStorageDeviceAttachment`, `VZVirtioBlockDeviceConfiguration` | 11.0 |
| Shared directories | `VZVirtioFileSystemDeviceConfiguration`, `VZSharedDirectory`, `VZSingleDirectoryShare` | 12.0 |
| Guest control transport | `VZVirtioSocketDeviceConfiguration`, `VZVirtioSocketDevice`, `VZVirtioSocketConnection` | 11.0 |
| Networking | `VZVirtioNetworkDeviceConfiguration`, `VZNATNetworkDeviceAttachment`, `VZFileHandleNetworkDeviceAttachment` | 11.0 |
| Balloon and entropy | `VZVirtioTraditionalMemoryBalloonDeviceConfiguration`, `VZVirtioEntropyDeviceConfiguration` | 11.0 |
| Serial console | `VZVirtioConsoleDeviceSerialPortConfiguration`, `VZFileHandleSerialPortAttachment` | 11.0 |

Apple documents the executable entitlement and the Linux VM setup. The spike
configured both NAT and a connected datagram file-handle attachment. It covers
configuration and guest driver binding, with network traffic and storage
behavior measured by their owning tasks. [Apple Linux setup](https://developer.apple.com/documentation/virtualization/running-linux-in-a-virtual-machine) · [Virtualization entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.virtualization) · [File-handle network attachment](https://developer.apple.com/documentation/virtualization/vzfilehandlenetworkdeviceattachment)

[Harness and reproduction commands](../../spikes/vz-boot/README.md)
