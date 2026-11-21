# M1 spike review

The spikes used macOS 27.2.1 on Apple silicon. The reports contain the test setup, measurements, and commands.

| Task | Findings |
|---|---|
| [M1-01 GPUI right-to-left](M1-01-gpui-rtl.md) | Four of five criteria passed. Arabic single-line input placed the caret and pointer clicks at incorrect positions. Correcting the defect spans GPUI text geometry and GPUI Kit input. |
| [M1-02 Apple virtualization bridge](M1-02-vz-binding.md) | Direct Rust bindings and the Swift bridge both booted Alpine, reached the serial shell, exchanged a random vsock challenge, and reported guest shutdown. Both validated the planned device configuration. |
| [M1-03 Disk reclaim](M1-03-disk-reclaim.md) | Guest `fstrim` deallocated about 4 GiB from each tested raw APFS-backed image. Retained file checksums and offline filesystem checks passed. The storage design selects automatic caching with full synchronization. |
| [M1-04 Virtiofs](M1-04-virtiofs.md) | Host edits became readable in the guest without a guest watch event. The guest-origin control fired. Warm throughput was 2.62 GiB/s through virtiofs and 15.33 GiB/s through native APFS. The tested non-root guest UID could not create a file. |

## Gate status

- **G1. Rust UI required.** The owner rejected Tauri 2 and confirmed that Marsa should use a Rust UI framework. GPUI is the conditional candidate in [D-004](../15-decisions.md#d-004-gpui-for-the-gui-gated-by-a-right-to-left-spike). Its current Arabic caret and hit-testing results leave the gate open.
- **G2. Direct Rust bindings recommended.** Both bridges met the runtime criteria. The choice still needs explicit owner agreement. [Apple bridge comparison](../04-virtualization-macos.md#calling-the-framework-from-rust).

M4-06 owns the notification relay and real-container permission validation. M9-03 verifies disk reclaim through the production path.
