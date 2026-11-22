# M11 Performance

**Outcome:** reproducible benchmarks, a working `krun` backend, and an evidence-based choice of the default backend.

Design: [13 Benchmarks](../13-testing-and-benchmarks.md#benchmarks), [04 Choosing the default](../04-virtualization-macos.md#choosing-the-default-the-benchmark-gate), D-008.

---

### M11-01 Benchmark harness

- **Status:** todo
- **Depends on:** M4-14
- **Docs:** [13 What is measured](../13-testing-and-benchmarks.md#what-is-measured)
- **Do:** Implement `cargo xtask bench` covering every benchmark in the table, with fixed inputs, warm-up rules, repeated runs and variance reported. Store results per commit and compare against the previous run with regression thresholds.
- **Deliver:** `tests/bench/`, result storage, a regression check in the Tier 2 `main` workflow.
- **Accept when:**
  - Every benchmark in the table produces a number with variance.
  - A deliberate slowdown trips the regression check.
  - The method is documented well enough for someone else to reproduce it on the same hardware.

### M11-02 libkrun spike

- **Status:** done
- **Depends on:** M3-01, M2-10
- **Docs:** [04 The krun backend](../04-virtualization-macos.md#the-krun-backend), V-09
- **Do:** Verify the current libkrun release, licence, macOS requirements, API and kernel requirements (whether it needs its own bundled kernel). Boot the Marsa guest image with it, or document exactly what must change in the guest for it to boot. Check virtiofs, vsock, block and network device support against `VmSpec`.
- **Deliver:** `spikes/krun-boot/`, `docs/spikes/M11-02-libkrun.md`, an updated V-09 entry.
- **Accept when:**
  - The report states whether every `VmSpec` field can be satisfied and at what cost.
  - It recommends proceed or stop, with reasons.

**Verification.** The signed probe booted the unchanged Marsa image and checked its block, console, vsock, virtiofs and disconnected Ethernet devices. The [report](../spikes/M11-02-libkrun.md) recommends stopping until supported macOS isolation can enforce the shared-folder allowlist.

### M11-03 `krun` backend

- **Status:** blocked (libkrun requires a supported macOS share isolation boundary)
- **Depends on:** M11-02
- **Docs:** [04 The krun backend](../04-virtualization-macos.md#the-krun-backend)
- **Do:** If M11-02 recommends proceeding, implement `marsa-vm-krun` behind the Hypervisor trait with accurate `Capabilities`, selectable through the `vm.backend` setting, with the hypervisor entitlement added to signing.
- **Deliver:** `crates/marsa-vm-krun`, Tier 2 tests parameterised over both backends.
- **Accept when:**
  - The boot test and the conformance suite pass with `backend = "krun"`.
  - Switching backends requires only the setting and a VM restart; the data disk works with both.

### M11-04 Backend gate evaluation

- **Status:** todo
- **Depends on:** M11-01, M11-02, M9-10, and M11-03 if the spike recommends proceeding
- **Docs:** [04 Choosing the default](../04-virtualization-macos.md#choosing-the-default-the-benchmark-gate)
- **Do:** Run every gate measure on both backends on the reference Mac, including the 72-hour soak for `krun`. Write the comparison and record the default-backend decision. If M11-02 or M11-03 stopped early, record that `vz` remains the default and why.
- **Deliver:** `docs/spikes/M11-04-backend-gate.md`, a decision record.
- **Accept when:**
  - If both backends qualify, each gate measure has numbers for both. If the spike recommends stopping, report the Virtualization.framework measurements and the verified reason a comparison is unavailable.
  - The decision record names the default backend for version 1 and has the owner's agreement.

### M11-05 Comparative benchmarks

- **Status:** todo
- **Depends on:** M11-01
- **Docs:** [13 Comparisons](../13-testing-and-benchmarks.md#comparisons)
- **Do:** Run the same benchmarks against Docker Desktop and at least one other alternative on the same machine, with exact versions, settings and method recorded. Replace any planning-stage numbers quoted from other projects with these measurements wherever the docs cite them.
- **Deliver:** `docs/reference/benchmarks.md` with the results and method.
- **Accept when:**
  - Every comparison states versions, hardware, settings and date.
  - Marsa's success measures in [01](../01-vision-and-scope.md#success-measures) are each marked met or not met with the measured value.

### M11-06 Performance tuning pass

- **Status:** todo
- **Depends on:** M11-05
- **Docs:** [01 Success measures](../01-vision-and-scope.md#success-measures)
- **Do:** For each success measure not yet met, profile, fix and re-measure: boot stages, port-forwarder overhead, file-sharing throughput, GUI start-up and log-stream frame time, idle memory. Stop when all measures are met or the remaining gap is explained and accepted by the owner.
- **Deliver:** fixes, updated benchmark results.
- **Accept when:**
  - Every success measure is met, or has a written, owner-accepted exception.
