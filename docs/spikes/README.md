# M1 spike review

The spikes used macOS 27.2.1 on Apple silicon. The reports contain the test setup, measurements, and commands.

| Task | Findings |
|---|---|
| [M1-01 GPUI right-to-left](M1-01-gpui-rtl.md) | Four of five criteria passed. Arabic single-line input placed the caret and pointer clicks at incorrect positions. Correcting the defect spans GPUI text geometry and GPUI Kit input. |
| [M1-02 Apple virtualization bridge](M1-02-vz-binding.md) | Direct Rust bindings and the Swift bridge both booted Alpine, reached the serial shell, exchanged a random vsock challenge, and reported guest shutdown. Both validated the planned device configuration. |
| [M1-03 Disk reclaim](M1-03-disk-reclaim.md) | Guest `fstrim` deallocated about 4 GiB from each tested raw APFS-backed image. Retained file checksums and offline filesystem checks passed. The storage design selects automatic caching with full synchronization. |
| [M1-04 Virtiofs](M1-04-virtiofs.md) | Host edits became readable in the guest without a guest watch event. The guest-origin control fired. Warm throughput was 2.62 GiB/s through virtiofs and 15.33 GiB/s through native APFS. The tested non-root guest UID could not create a file. |

## Decisions awaiting owner agreement

- **G1. Use the Tauri 2 fallback.** The Arabic input failure meets the fallback condition in [D-004 and D-005](../15-decisions.md#d-004-gpui-for-the-gui-gated-by-a-right-to-left-spike). M6 and M7 retain the four language and appearance variants, Figma designs, and keyboard and accessibility criteria.
- **G2. Use the direct Rust Virtualization.framework bindings.** Both bridges reached the same runtime checks. The direct binding keeps the system API integration in Rust and avoids a second build and cross-language callback boundary. [Apple bridge decision rule](../04-virtualization-macos.md#calling-the-framework-from-rust).

M4-06 owns the notification relay and real-container permission validation. M9-03 verifies disk reclaim through the production path. M1-05 records the owner's decisions before dependent implementation starts.
