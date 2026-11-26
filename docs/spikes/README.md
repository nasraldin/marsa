# M1 spike review

The spikes used macOS 27.2.1 on Apple silicon. The reports contain the test setup, measurements, and commands.

| Task | Findings |
|---|---|
| [M1-01 GPUI right-to-left](M1-01-gpui-rtl.md) | Four of five criteria passed. Arabic single-line input placed the caret and pointer clicks at incorrect positions. Correcting the defect spans GPUI text geometry and GPUI Kit input. |
| [M1-02 Apple virtualization bridge](M1-02-vz-binding.md) | Direct Rust bindings and the Swift bridge both booted Alpine, reached the serial shell, exchanged a random vsock challenge, and reported guest shutdown. Both validated the planned device configuration. |
| [M1-03 Disk reclaim](M1-03-disk-reclaim.md) | Guest `fstrim` deallocated about 4 GiB from each tested raw APFS-backed image. Retained file checksums and offline filesystem checks passed. The storage design selects automatic caching with full synchronization. |
| [M1-06 GPUI desktop qualification](M1-06-gpui-qualification.md) | The native AppKit editor passed Arabic input, native arrows, mouse selection and live IME. A shared AppKit parent exposed the native and GPUI accessibility controls. |
| [M1-04 Virtiofs](M1-04-virtiofs.md) | Host edits became readable in the guest without a guest watch event. The guest-origin control fired. Warm throughput was 2.62 GiB/s through virtiofs and 15.33 GiB/s through native APFS. The tested non-root guest UID could not create a file. |

## Gate status

- **G1. GPUI selected.** The owner confirmed GPUI in [D-023](../15-decisions.md#d-023-choose-gpui-for-the-desktop). The desktop qualification verified the native editor for Arabic input, native arrow events, live IME conversion and the macOS accessibility tree. M6-08 still owns physical keyboard and spoken VoiceOver validation. [Desktop qualification](M1-06-gpui-qualification.md).
- **G2. Direct Rust bindings selected.** The owner approved generated `objc2-virtualization` bindings after both bridges met the runtime criteria. [D-024](../15-decisions.md#d-024-use-direct-rust-bindings-for-virtualizationframework).

M4-06 enforces bind-mount policy. M4-19 qualified host edits and container permissions with scoped Compose Watch under D-031. M9-03 verifies disk reclaim through the production path.
