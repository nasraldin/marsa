# Virtualization.framework boot spike

This standalone workspace compares direct Rust bindings with a Swift C ABI
bridge. Both routes use the same Rust runner, stock guest kernel, initramfs,
and runtime assertions. The spike is independent of the shipped workspace.
[Observed results](../../docs/spikes/M1-02-vz-binding.md)

## Run on Apple silicon

Use a physical Apple silicon Mac running macOS 27 or later, Xcode with Swift
6.3 or later, Python 3, and the repository's pinned Rust toolchain. Run from
this directory as the current user.

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
  cargo build --locked --release --target aarch64-unknown-linux-musl -p marsa-vsock-spike-guest

scratch=$(mktemp -d)
python3 fixtures.py target/aarch64-unknown-linux-musl/release/marsa-vsock-spike-guest \
  "$scratch/fixtures"

MACOSX_DEPLOYMENT_TARGET=27.0 cargo build --locked -p marsa-vz-spike --features swift
codesign --force --sign - --timestamp=none --entitlements entitlement.plist target/debug/marsa-vz-spike
codesign --verify --strict target/debug/marsa-vz-spike

target/debug/marsa-vz-spike rust "$scratch/fixtures" "$scratch/rust-console.log"
target/debug/marsa-vz-spike swift "$scratch/fixtures" "$scratch/swift-console.log"
```

`fixtures.py --iso <path>` reuses a downloaded Alpine 3.24.4 ARM64 virt ISO.
Preparation copies a supplied ISO to a private temporary directory and
verifies that copy and its decompressed kernel before use. The archive and generated guest image belong in the
temporary directory. The macOS command opens no host listener. The guest echo
listener is vsock-only and exits after one connection.

Build without `--features swift` for the direct Rust route with no Swift build
step. Reapply the entitlement after rebuilding the executable.

## Reuse the harness

To verify the Marsa kernel with built-in devices, run `cargo xtask guest kernel`
from the repository root. Add `--kernel ../../target/guest/Image` to the
`fixtures.py` command above. The fixture then skips loading the stock Alpine
modules and still verifies block devices, virtiofs and vsock through the same
signed Rust runner.

The runner accepts an optional fourth argument naming a local shell script of
up to 16 KiB, with individual lines up to 2,048 bytes. It sends that script to the guest's interactive prompt after the
vsock check, records its output in the selected transcript, and waits for the
guest shutdown callback. Each run requires a new transcript filename and has a
90-second deadline. The runner creates transcripts with owner-only permissions.

The verified Alpine module image is the guest's read-only `/dev/vda`. The
writable `/dev/vdb` is the fixture directory's sparse `data.img`. Prepare
the fixture before resizing or replacing that image for a storage experiment.
The read-only virtiofs tag `marsa-spike` exposes the fixture directory's
`share/` child. The setup script unmounts the verification mount before the
test shell, so another script can mount it with its own mount options. Use a
fresh, owner-only fixture directory and temporary data for each experiment.
Keep the fixture tree under your control for the whole run. The framework
opens the checked files by path.

## Virtiofs correctness and timings

Build optimized native and Linux versions of the same filesystem probe.
The host runner still needs the virtualization entitlement after rebuilding.

```sh
MACOSX_DEPLOYMENT_TARGET=27.0 cargo build --locked --release -p marsa-vz-spike -p marsa-virtiofs-spike
codesign --force --sign - --timestamp=none --entitlements entitlement.plist target/release/marsa-vz-spike
codesign --verify --strict target/release/marsa-vz-spike
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
  cargo build --locked --release --target aarch64-unknown-linux-musl \
  -p marsa-vsock-spike-guest -p marsa-virtiofs-spike

scratch=$(mktemp -d)
set -o pipefail
python3 -u virtiofs.py correctness "$scratch/check" | tee "$scratch/check.txt"
python3 -u virtiofs.py benchmark "$scratch/timing-1" --first native | tee "$scratch/timing-1.txt"
python3 -u virtiofs.py benchmark "$scratch/timing-2" --first guest | tee "$scratch/timing-2.txt"
python3 -u virtiofs.py benchmark "$scratch/timing-3" --first native | tee "$scratch/timing-3.txt"
```

Pass `--iso <path>` to reuse the same downloaded Alpine image. The benchmark
creates and ejects only its temporary APFS images. Keep the Mac idle during
timings. The correctness helper opts into `--writable-share` for its newly
created fixture only. The runner otherwise uses a read-only virtiofs share.
[Measurements and findings](../../docs/spikes/M1-04-virtiofs.md)

## Verify Marsa PID 1

Build the arm64 rootfs, kernel, static Linux `marsa-init` binary and vsock test
binary. Prepare a private fixture directory with `fixtures.py --kernel`. Keep
the original initramfs until the replacement is built.

```sh
python3 pid1/prepare.py ../../target/guest/rootfs.tar \
  ../../target/aarch64-unknown-linux-musl/release/marsa-init \
  target/aarch64-unknown-linux-musl/release/marsa-vsock-spike-guest \
  ../../target/guest/docker-29.10.0.tgz "$scratch/pid1-initramfs"
mv "$scratch/pid1-initramfs" "$scratch/fixtures/initramfs"
target/debug/marsa-vz-spike --via-init rust "$scratch/fixtures" \
  "$scratch/pid1-console.log" pid1/check.sh
```

The boot fixture substitutes a local readiness signal for the host handshake
that M2-06 provides. It exercises actual guest mounts, Docker and containerd,
orphan reaping and PID 1 shutdown. The runner sends SIGTERM to PID 1 and waits
for the Virtualization.framework shutdown callback.

## Verify the guest control agent

Build the static Linux `marsa-agent` binary with the workspace's pinned
[`protoc`](../../crates/marsa-proto/PROTOCOL.md#build-the-rust-bindings). Build
and sign the Rust bridge with real protocol support, then prepare a fresh
private fixture as for the PID 1 test.

```sh
MACOSX_DEPLOYMENT_TARGET=27.0 cargo build --locked -p marsa-vz-spike --features guest-control
codesign --force --sign - --timestamp=none --entitlements entitlement.plist target/debug/marsa-vz-spike
python3 pid1/prepare.py --control-agent ../../target/aarch64-unknown-linux-musl/release/marsa-agent \
  ../../target/guest/rootfs.tar ../../target/aarch64-unknown-linux-musl/release/marsa-init \
  target/aarch64-unknown-linux-musl/release/marsa-vsock-spike-guest \
  ../../target/guest/docker-29.10.0.tgz "$scratch/agent-initramfs"
mv "$scratch/agent-initramfs" "$scratch/fixtures/initramfs"
target/debug/marsa-vz-spike --via-init --guest-control rust "$scratch/fixtures" \
  "$scratch/agent-console.log" pid1/check.sh
```

This mode checks the guest's bounded `Hello` frame on vsock port 5000, selects
protocol 1.0, sends `SetTime`, and waits for its successful result and a Docker
readiness event. The guest console must report the set epoch from `date +%s`
and from the readiness file's modification time before the runner requests
shutdown. The wrapper starts the serial shell and forwards signals to the
agent. The guest control agent creates the readiness file.
