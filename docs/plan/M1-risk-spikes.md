# M1 Risk spikes

**Outcome:** the four unknowns that could change the architecture are answered with evidence before feature work starts.

Spike code lives under `spikes/` and is never shipped. Each spike ends with a written report in `docs/spikes/` stating what was tried, what was measured, and the answer.

---

### M1-01 Right-to-left spike in GPUI

- **Status:** todo
- **Depends on:** M0-01
- **Docs:** [08 The right-to-left risk](../08-gui.md#the-right-to-left-risk), [08 Spike pass criteria](../08-gui.md#spike-pass-criteria-task-m1-01), D-004, D-005, V-01
- **Do:** First, read the current state of right-to-left support upstream (GPUI tracking issues and merged changes) and pin the GPUI and gpui-component revisions you test. Build a small app with: a mirrored sidebar and list; a text field for Arabic input; rows mixing Arabic labels with English container names, image tags and paths; a runtime language switch; light and dark themes. Use a prototype of the start and end layout helpers. Test each of the five pass criteria and capture screenshots.
- **Deliver:** `spikes/gpui-rtl/`, `docs/spikes/M1-01-gpui-rtl.md` with a pass or fail per criterion, screenshots in four variants, the pinned revisions, and a list of any GPUI patches needed with an estimate of whether they are upstreamable.
- **Accept when:**
  - Every criterion has a clear pass or fail with evidence.
  - The report states one recommendation: proceed with GPUI, proceed with named patches, or switch to the Tauri fallback.
  - The upstream status found is written into [08](../08-gui.md).

### M1-02 Boot a Linux VM with Virtualization.framework from Rust

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [04 The vz backend](../04-virtualization-macos.md#the-vz-backend), V-02, V-03
- **Do:** Verify the framework classes and entitlement against Apple's current documentation for the minimum macOS. Boot a stock arm64 Linux kernel and a small initramfs to a serial console twice: once through Rust bindings and once through a minimal Swift shim with a C ABI. Attach a virtio block device, a vsock device and a console. Deliver VM state-change callbacks to Rust. Compare the two approaches on completeness, code size, build complexity and callback handling.
- **Deliver:** `spikes/vz-boot/`, `docs/spikes/M1-02-vz-binding.md` with the comparison and a recommendation.
- **Accept when:**
  - The VM boots to a shell prompt on the console with at least one approach, from an ad hoc signed binary carrying the entitlement.
  - A vsock round trip between host and guest works.
  - The report recommends bindings or shim using the decision rule in [04](../04-virtualization-macos.md#calling-the-framework-from-rust).
  - V-03 is resolved in the docs with the verified class list.

### M1-03 Prove disk space is returned to macOS

- **Status:** in progress
- **Depends on:** M1-02
- **Docs:** [07 Returning space to macOS](../07-file-sharing-and-storage.md#returning-space-to-macos), V-05, R-14
- **Do:** Using the spike VM, attach a sparse raw image as a data disk, format it ext4, write several gigabytes, delete them, run `fstrim`, and measure the image file's real on-disk size on APFS before and after. Try the available disk attachment options. If discard does not propagate, test offline compaction as the fallback.
- **Deliver:** `docs/spikes/M1-03-disk-reclaim.md` with measurements and the chosen mechanism.
- **Accept when:**
  - The report shows measured before and after sizes.
  - It states which mechanism Marsa will use and updates [07](../07-file-sharing-and-storage.md) accordingly.

### M1-04 Measure virtiofs sharing

- **Status:** todo
- **Depends on:** M1-02
- **Docs:** [07 Correctness requirements](../07-file-sharing-and-storage.md#correctness-requirements), V-06
- **Do:** Share a host directory into the spike VM with virtiofs. Measure small-file read latency, large-file throughput and a cold directory walk against native APFS. Check: file ownership seen in the guest and on the host for files created each way, permission bits, symlinks, and whether file-change notifications reach a watcher in the guest.
- **Deliver:** `docs/spikes/M1-04-virtiofs.md` with numbers and observed semantics.
- **Accept when:**
  - Each correctness requirement has an observed behaviour recorded.
  - The report states whether a host-side change forwarder is needed for hot reload.
  - Preliminary numbers are compared with the target in [01](../01-vision-and-scope.md#success-measures).

### M1-05 Spike review and gate decisions

- **Status:** todo
- **Depends on:** M1-01, M1-02, M1-03, M1-04
- **Docs:** [Plan gates](README.md#gates), [15 Decision records](../15-decisions.md)
- **Do:** Summarise the four reports for the project owner. Record gate G1 (GUI framework) and G2 (Apple bridge) as new decision records. Update every design doc and task that the findings change. If the Tauri fallback is chosen, rewrite the technology notes in M6 and M7 while keeping task IDs and acceptance criteria.
- **Deliver:** new decision records, updated docs and plan, `docs/spikes/README.md` indexing the reports.
- **Accept when:**
  - G1 and G2 are recorded with the owner's agreement noted.
  - No doc still carries a **Verify** label for V-01, V-02, V-03 or V-05.
  - The plan reflects any changed tasks.
