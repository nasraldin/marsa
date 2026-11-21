# Run the native VZ system test

The ignored test boots the packaged Linux kernel and EROFS image on a physical
Apple silicon Mac. Run it with macOS 27 or later from the repository root.
Build the guest with `cargo xtask guest all`, then provide the directory
containing `Image` and `rootfs.img`.

```sh
export MARSA_VZ_GUEST_DIR="$PWD/target/guest"
test_executable=$(
  MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsa-vm-vz \
    --test real_vm --no-run --message-format=json |
  python3 -c 'import json,sys; records=[json.loads(line) for line in sys.stdin]; print(next(r["executable"] for r in records if r.get("reason") == "compiler-artifact" and r["target"]["name"] == "real_vm" and r.get("executable")))'
)
codesign --force --sign - --timestamp=none \
  --entitlements spikes/vz-boot/entitlement.plist "$test_executable"
codesign --verify --strict "$test_executable"
"$test_executable" --ignored --nocapture
```

The test uses disposable sparse disks. It observes the packaged `marsa-init`
mounting system filesystems and preparing the data disk. It checks ordered
start and requested-stop events, a guest-initiated power-off reported as
an unexpected exit, and teardown after the last VM handle is dropped.
