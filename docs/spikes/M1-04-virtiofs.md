# M1-04 macOS virtiofs measurements

Host edits and renames reached the guest's file contents but generated no
guest inotify events. Event-driven hot reload needs a host-side change
forwarder or a polling fallback on this backend. The guest also refused the
tested non-root UID's write. M4-06 needs to verify a solution with real
container users before promising equivalent host and container permissions.

## Setup

Tested 2026-11-21 on an Apple M1 Max with 64 GiB RAM and macOS 27.2.1. The
signed direct Rust harness used the stock Alpine 3.24.4 arm64 fixture and
Linux 6.18.60-0-virt from M1-02. The VM had two vCPUs and 1 GiB RAM.
Correctness tests used a new writable scratch folder on the Mac's APFS data
volume. The host ran without elevated privileges. The benchmark shared only
its separate scratch volume read-only. Both bridges also rejected direct writes when the opt-in was omitted. [M1-02 fixture](M1-02-vz-binding.md) · [Apple shared directory configuration](https://developer.apple.com/documentation/virtualization/shared-directories)

## Correctness

| Check | Observed result |
|---|---|
| Host creates a file | macOS reported owner `501:20`, mode `0640`. Linux reported `0:0`, mode `0640`. |
| Guest root creates a file | Linux reported owner `0:0`, mode `0710`. macOS reported the Mac user's `501:20`, mode `0710`. |
| Guest non-root creates a file | After dropping supplementary groups and changing to UID/GID `12345:12345`, creation returned `EACCES`. Linux showed the target directory as `0777` and the shared mount root as `0755`. No file appeared on the Mac. |
| Case | The lowercase lookup resolved the same mixed-case name on native APFS and in the guest. |
| Symlinks | Each side read the other side's relative link target and its file contents. A relative dangling link remained a symlink and failed lookup on both sides. The observed link mode was `0755`. |
| Hard links | A host-created pair had the same guest inode and link count 2. A guest-created pair had the same host inode and link count 2. |
| Extended attributes | The Linux system calls and macOS `xattr` both read the other side's exact `user.marsa.spike` value. |
| Guest-origin change notifications | After watches were installed, a guest append generated two directory events and two file events in the same virtiofs directory used for the host cases. |
| Host-origin change notifications | After the guest signaled readiness, the Mac appended to an existing watched file and renamed a temporary file into the watched directory. The guest observed zero file or directory events for these operations over five seconds. It read both exact updated values afterward. |

The guest watch covered create, modify, close-write, move-from, and move-to
events. The host only changed the intended scratch files. The result
distinguishes missing notifications from missing file-content coherence. M4-06
must still test the relay and permission behavior in a real Docker container
and a dev server.

## Performance

Each round created two distinct 512 MiB raw APFS images with identical
deterministic datasets. One mounted in macOS for the native measurement and
the other provided the virtiofs backing folder. Both were ejected after
population and mounted read-only before timing. The same release Rust probe
ran natively and in Linux, and execution order was native first, guest first,
then native first. No other Marsa VM or disk-reclaim test ran during these
measurements.

The metadata walk was the first access to its tree after remount and visits
128 directories with 128 regular files each, performing a metadata check per
file. Ejecting and remounting makes this a fresh filesystem traversal. The
underlying host storage caches were not flushed. The other workloads were
deliberately warm. The small-file workload opens, reads, and closes 2,048
4 KiB files in a fixed permutation, skips one warm-up pass, and records five
passes per round. The bulk workload reads a 128 MiB file using a 1 MiB buffer,
skips one warm-up read, and records five reads per round.

| Workload | Native APFS | Guest virtiofs | Relative cost |
|---|---:|---:|---:|
| 4 KiB open/read/close, median of three p50s | 33.0 µs | 176.4 µs | 5.34× latency |
| 4 KiB open/read/close, median of three p95s | 41.9 µs | 219.7 µs | 5.24× latency |
| Sequential 128 MiB read, median of 15 runs | 8.15 ms, 15.33 GiB/s | 47.77 ms, 2.62 GiB/s | 5.86× elapsed time |
| First metadata walk, median of three mounts | 200 ms | 1,039 ms | 5.19× elapsed time |

The three native metadata walks took 200.3, 169.5, and 202.2 ms. The guest
walks took 1,051.9, 1,039.0, and 994.3 ms. Native sequential read samples
ranged from 8.00 to 8.65 ms. Guest samples ranged from 41.25 to 50.12 ms.

The preliminary bulk result misses the [within 2× native APFS target](../01-vision-and-scope.md#success-measures).
These are cached read rates through scratch APFS disk images, so the test
does not estimate physical SSD throughput. Later application benchmarks
should include source trees and build artifacts on ordinary project storage.

[Reproduction commands](../../spikes/vz-boot/README.md#virtiofs-correctness-and-timings)
