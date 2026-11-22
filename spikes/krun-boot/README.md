# Reproduce the libkrun boot qualification

Use a standard account on an Apple silicon Mac with macOS 27, Xcode command-line tools, the workspace's pinned stable Rust and protobuf tools, and Python 3.11 or later. Assemble the normal guest with `cargo xtask guest all` first. The fixture uses a new private 1 GiB sparse disk, an empty read-only directory and a disconnected Ethernet socket. It leaves its own logs in the supplied output directory.

From the Marsa repository root:

```sh
marsa_krun_scratch=$(mktemp -d /tmp/marsa-krun.XXXXXXXX)
git clone --depth 1 --branch v1.19.8 \
  https://github.com/libkrun/libkrun.git "$marsa_krun_scratch/source"
test "$(git -C "$marsa_krun_scratch/source" rev-parse HEAD)" = \
  c4d283c233bc0ae1993c289ec2a1b4da2242f1f8 || exit 1
(
  cd "$marsa_krun_scratch/source" &&
  MACOSX_DEPLOYMENT_TARGET=27.0 cargo +1.100.0 build --locked \
    -p libkrun --no-default-features --features blk,net
) || exit 1

marsa_krun_library="$marsa_krun_scratch/source/target/debug"
marsa_krun_launcher="$marsa_krun_scratch/krun-boot"
ln -s libkrun.dylib "$marsa_krun_library/libkrun.1.dylib"
clang -arch arm64 -mmacosx-version-min=27.0 -Wall -Wextra -Werror \
  -I "$marsa_krun_scratch/source/include" -L "$marsa_krun_library" \
  -Wl,-rpath,"$marsa_krun_library" -lkrun spikes/krun-boot/boot.c \
  -o "$marsa_krun_launcher" || exit 1
install_name_tool -change libkrun.1.dylib @rpath/libkrun.1.dylib \
  "$marsa_krun_launcher" || exit 1
codesign --force --sign - --timestamp=none --entitlements \
  spikes/krun-boot/entitlement.plist "$marsa_krun_launcher" || exit 1
codesign --verify --strict "$marsa_krun_launcher" || exit 1
python3 spikes/krun-boot/probe.py "$marsa_krun_launcher" \
  target/guest "$marsa_krun_scratch/result"
```

Success produces a signed-run boot log, a decoded guest greeting, and the fixture disk under the result directory. See the [qualification](../../docs/spikes/M11-02-libkrun.md) for the security and device findings.
