# M2 Guest image

**Outcome:** a reproducible, minimal Linux image (kernel plus read-only root filesystem) that boots to a healthy Docker engine, with `marsa-init` and `marsa-agent` inside.

Design: [05 Guest image](../05-guest-image.md).

---

### M2-01 Guest build environment and version pins

- **Status:** done
- **Depends on:** M0-03
- **Docs:** [05 Build pipeline](../05-guest-image.md#build-pipeline)
- **Do:** Create `guest/versions.toml` pinning the kernel release, the Alpine release and package list, and the Docker static-binary version, each with a checksum. Implement `cargo xtask guest` to run the build inside a container so it works on a Mac and in CI. Verify the current upstream versions before pinning.
- **Deliver:** `guest/versions.toml`, the build container definition, `xtask guest` with `kernel`, `rootfs`, `image` and `all` subcommands as a working skeleton.
- **Accept when:**
  - `cargo xtask guest all` runs end to end on a Mac and in Tier 1 CI, producing verified source inputs for the kernel and root filesystem tasks.
  - Every download is verified against its pinned checksum; a wrong checksum fails the build.

### M2-02 Minimal kernel

- **Status:** done
- **Depends on:** M2-01
- **Docs:** [05 Kernel](../05-guest-image.md#kernel)
- **Do:** Write the arm64 kernel config with only the options containers and the virtual devices need. Build it in the guest build. Run Docker's `check-config.sh` against the config in the build and fail on missing required options.
- **Deliver:** `guest/kernel/config`, the kernel build step, the `vmlinux` output.
- **Accept when:**
  - `check-config.sh` reports no missing "Generally Necessary" items.
  - The kernel boots in the M1-02 spike VM and reaches init.
  - The config contains no physical hardware, sound, graphics or wireless drivers.

### M2-03 Root filesystem recipe

- **Status:** done
- **Depends on:** M2-01
- **Docs:** [05 Root filesystem contents](../05-guest-image.md#root-filesystem-contents), V-15
- **Do:** Assemble the Alpine-based root filesystem with only the packages in the contents table. Add the pinned upstream Docker static binaries unmodified. Verify which networking packages the pinned engine needs (V-15) and record it. Write the engine configuration (`daemon.json`) with the defaults from the security doc.
- **Deliver:** `guest/rootfs/` recipe, engine configuration, an updated V-15 entry.
- **Accept when:**
  - The tree contains no package manager cache, no shell history, no SSH server and no unlisted services.
  - Engine binaries match upstream checksums byte for byte.
  - A size report is printed by the build and stored as a CI artifact.

### M2-04 Read-only image and manifest

- **Status:** done
- **Depends on:** M2-02, M2-03
- **Docs:** [05 Layout](../05-guest-image.md#layout), V-17
- **Do:** Verify EROFS support in the pinned kernel and build tools (V-17); use SquashFS if EROFS is not workable. Build `rootfs.img` and generate `guest-manifest.json` with every component version and the checksums of kernel and image.
- **Deliver:** the image step, the manifest generator, the manifest schema in `marsa-core`.
- **Accept when:**
  - The kernel mounts the image read-only as root.
  - `marsa-core` parses and validates the manifest; a mismatched checksum is detected by a test.
  - The chosen format is recorded in the decision records.

### M2-05 Implement `marsa-init`

- **Status:** done
- **Depends on:** M0-04, M2-02
- **Docs:** [05 marsa-init](../05-guest-image.md#marsa-init)
- **Do:** Implement PID 1 as a static musl binary following the boot flow: mounts, cgroup v2, tmpfs overlays, data-disk create or check, grow, mount, virtiofs shares, network, start the agent, apply host config, start containerd and dockerd, supervise and reap, clean shutdown. Read variable input only from the kernel command line and the agent.
- **Deliver:** `crates/marsa-init` with unit tests for the parsing and sequencing logic.
- **Accept when:**
  - The spike VM boots with `marsa-init` as PID 1 and prints each stage with its duration.
  - An unformatted data disk is formatted; a dirty one is checked; an enlarged one is grown.
  - Shutdown stops the engine, unmounts the data disk cleanly and powers off; the next boot needs no filesystem repair.
  - Orphaned processes are reaped (tested).

### M2-06 `marsa-agent`: handshake, time and health

- **Status:** done
- **Depends on:** M0-05, M2-05
- **Docs:** [05 marsa-agent](../05-guest-image.md#marsa-agent)
- **Do:** Implement the agent's vsock listener, the versioned handshake with boot timings, time setting, and engine health reporting based on the engine's ping endpoint, including restart events.
- **Deliver:** `crates/marsa-agent` with the first three functions and tests against a loopback transport.
- **Accept when:**
  - The handshake carries protocol, image and component versions.
  - Setting the time changes the guest clock (tested in the VM).
  - Health transitions are reported within one second of the engine becoming ready or failing.
  - Malformed and oversized messages are rejected without crashing.

### M2-07 `marsa-agent`: network, proxy and certificates

- **Status:** done
- **Depends on:** M2-06
- **Docs:** [06 DNS](../06-networking.md#dns), [06 Proxies and certificates](../06-networking.md#proxies-and-certificates)
- **Do:** Implement applying DNS servers and search domains, proxy settings for dockerd and BuildKit, extra hosts for `host.docker.internal` and related names, and installing host CA certificates into the guest trust store before the engine starts. Changes after boot must apply without a VM restart where the engine allows it.
- **Deliver:** agent handlers and tests.
- **Accept when:**
  - A pushed CA certificate is trusted by an image pull from a registry using that CA (tested with a local registry).
  - New containers use DNS changes pushed after boot.
  - Docker starts with the configured proxy. Runtime proxy changes are staged and report that a restart is required. M4-15 owns restart orchestration.
  - A container with Docker's `host-gateway` entry resolves `host.docker.internal` to the configured address. M4-08 owns injection for unmodified clients.

### M2-08 `marsa-agent`: ports, stats, disk, shares, shutdown and API bridge

- **Status:** done
- **Depends on:** M2-06
- **Docs:** [05 marsa-agent](../05-guest-image.md#marsa-agent), [06 Published ports](../06-networking.md#published-ports)
- **Do:** Implement port open and close events from engine events and container inspection, the VM stats stream, `fstrim` with a result report, mounting and unmounting virtiofs shares on request, clean shutdown, and the bridge from a second vsock port to dockerd's Unix socket.
- **Deliver:** agent handlers and tests.
- **Accept when:**
  - Publishing and releasing ports produces exactly one open and one close event each, including for container restarts.
  - The Docker API bridge supports streaming and connection upgrades (attach and exec work through it).
  - Trim reports bytes trimmed.
  - A shutdown request stops the engine and powers off within the timeout.

### M2-09 Reproducible build, SBOM and source bundle

- **Status:** in progress
- **Depends on:** M2-04, M2-05, M2-08
- **Docs:** [05 Licence obligations](../05-guest-image.md#licence-obligations), [11 Updates and supply chain](../11-security.md#updates-and-supply-chain)
- **Do:** Make the guest build bit-for-bit reproducible. Generate an SBOM for the image and a source bundle reference (exact kernel source, config, Alpine package sources) with licence texts.
- **Deliver:** reproducibility settings, SBOM and notices generation in `xtask guest`.
- **Accept when:**
  - CI builds the image twice and the checksums match.
  - The SBOM lists every package and binary with version and licence.
  - The kernel source reference and config are emitted as release artifacts.

### M2-10 Guest boot test

- **Status:** done
- **Depends on:** M2-04, M2-05, M2-06, M2-09
- **Docs:** [13 System tests](../13-testing-and-benchmarks.md#system-tests-real-vm)
- **Do:** Add a system test that boots the built image in the spike harness (later replaced by `marsad`), waits for the agent handshake and engine health, runs `hello-world`, and shuts down cleanly. Record boot stage timings as a CI artifact.
- **Deliver:** a Tier 2 test and its harness.
- **Accept when:**
  - The test passes on the virtualization runner.
  - Boot timings are reported against the budget in [04](../04-virtualization-macos.md#boot-sequence).
