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
Preparation verifies the official SHA-256 and the decompressed kernel digest
before using them. The archive and generated guest image belong in the
temporary directory. The macOS command opens no host listener. The guest echo
listener is vsock-only and exits after one connection.

Build without `--features swift` for the direct Rust route with no Swift build
step. Reapply the entitlement after rebuilding the executable.

## Reuse the harness

The runner accepts an optional fourth argument naming a local shell script of
up to 16 KiB, with individual lines up to 2,048 bytes. It sends that script to the guest's interactive prompt after the
vsock check, records its output in the selected transcript, and waits for the
guest shutdown callback. Each run requires a new transcript filename and has a
90-second deadline.

The verified Alpine module image is the guest's read-only `/dev/vda`. The
writable `/dev/vdb` is the fixture directory's sparse `data.img`. Prepare
the fixture before resizing or replacing that image for a storage experiment.
The read-only virtiofs tag `marsa-spike` exposes the fixture directory's
`share/` child. The setup script unmounts the verification mount before the
test shell, so another script can mount it with its own mount options. Use a
fresh fixture directory and temporary data for each experiment.
