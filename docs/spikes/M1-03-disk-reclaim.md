# M1-03 RAW disk reclaim on APFS

Guest-initiated discard returned approximately 4 GiB of actual allocation
to APFS in every tested cache mode. Marsa will run ext4 `fstrim` after
cleanup and during idle reclaim, using the sparse RAW virtio-blk attachment.
The production test in M9-03 will exercise the same path after Docker image
cleanup.

## Environment and method

- Tests ran on 2026-11-21 on a physical Apple silicon Mac, macOS 27.2.1
  build 26B131, with the test files on APFS. The host ran as the logged-in user.
- The [direct Rust bridge](M1-02-vz-binding.md) used a stock Alpine 3.24.4 ARM64
  kernel and module image, two vCPUs and 1 GiB of guest memory. Signed Alpine
  packages provided e2fsprogs 1.47.4-r0 and fstrim 2.42.3-r1.
- Each cache setting used a separate 8 GiB sparse RAW file and a read-write
  `VZDiskImageStorageDeviceAttachment` with full synchronization. macOS 27.2
  exposes automatic, cached and uncached cache settings. Holding full
  synchronization constant preserves the disk durability guarantee across
  the trials. [Apple disk image attachment options](https://developer.apple.com/documentation/virtualization/vzdiskimagestoragedeviceattachment)
- The guest formatted ext4 with initial discard and lazy initialization
  disabled and mounted it with `nodiscard`. It wrote 4 GiB of removable data
  and a 32 MiB retained file from `/dev/urandom`. It then deleted the 4 GiB
  file and called `fstrim --verbose /mnt/disk`. The host sampled the file's
  allocated bytes as `st_blocks × 512` at synchronized guest checkpoints and
  again after shutdown.

## Measured allocated bytes

The image started at zero allocated bytes and retained its 8,589,934,592-byte
logical size in every run. This table comes from the second three-mode run,
which also checked filesystem consistency and retained data.

| Cache mode | Formatted | Filled | After deletion | After trim | After shutdown | Released from deletion to shutdown |
|---|---:|---:|---:|---:|---:|---:|
| Automatic | 4,366,336 | 4,333,199,360 | 4,333,371,392 | 38,400,000 | 38,400,000 | 4,294,971,392 |
| Cached | 4,554,752 | 4,333,330,432 | 4,333,518,848 | 38,522,880 | 38,522,880 | 4,294,995,968 |
| Uncached | 4,366,336 | 4,333,199,360 | 4,333,371,392 | 38,400,000 | 38,400,000 | 4,294,971,392 |

Each guest reported a 512-byte discard granularity and an 8,589,934,592-byte
discard maximum. The trim command exited zero and reported 8,316,710,912 bytes
of unused ext4 blocks submitted for discard in each mode. The physical
recovery figure comes from the APFS allocation difference. The guest counter
can include sectors that were already unallocated. [util-linux 2.42.3 fstrim manual](https://github.com/util-linux/util-linux/blob/v2.42.3/sys-utils/fstrim.8.adoc)

In the second run, the retained file's SHA-256 matched before deletion and
after trim in all three modes. Read-only `e2fsck -fn` passed all five checks after the filesystem was
unmounted, and all six trials observed orderly VM shutdown callbacks.

## Selected mechanism

Use automatic caching with full synchronization and explicit `fstrim` on the
guest data filesystem. Measure physical allocation on the host for the
reported reclamation. The tested VZ path reclaimed space without an offline
copy, so the offline fallback remains untested.

This spike covers sparse RAW images on APFS with the tested macOS release.
M9-03 will verify the full product path. Any other VM backend must establish
its own reclaim behavior.

[Reproduction commands](../../spikes/disk-reclaim/README.md)
