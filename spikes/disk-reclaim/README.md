# RAW disk reclaim spike

Use the [direct Rust boot harness](../vz-boot/README.md) on a physical Apple
silicon Mac. Build it without the Swift feature and sign it as described
there. Prepare the stock boot fixtures with the verified Alpine 3.24.4 ARM64
virt ISO. Reuse that ISO for the storage tools.

From the repository root, use a fresh directory on APFS.

```sh
scratch=$(mktemp -d)
python3 -B spikes/disk-reclaim/run.py \
  spikes/vz-boot/target/debug/marsa-vz-spike \
  "$path_to_vz_fixtures" "$path_to_alpine_iso" "$scratch/reclaim"
```

The experiment creates a fresh 8 GiB sparse RAW file for each cache mode. Each
run writes 4 GiB of removable data and a separate 32 MiB retained file. Allow
at least 13 GiB of free space in case the host doesn't release any blocks.
The output directory contains the allocated-byte measurements and serial
logs. The harness uses only files created in this directory for the data disks.

[Results and interpretation](../../docs/spikes/M1-03-disk-reclaim.md)
