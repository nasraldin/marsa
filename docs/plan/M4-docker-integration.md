# M4 Docker integration

**Outcome:** the standard `docker` CLI, Compose, Buildx and common tools work against Marsa: socket, contexts, shared folders, data disk, networking and ports.

Design: [03 Docker socket proxy](../03-host-daemon.md#docker-socket-proxy), [06 Networking](../06-networking.md), [07 File sharing and storage](../07-file-sharing-and-storage.md), [09 CLI and Docker compatibility](../09-cli-and-docker-compat.md).

---

### M4-01 Docker socket proxy

- **Status:** todo
- **Depends on:** M3-07, M2-08
- **Docs:** [03 Docker socket proxy](../03-host-daemon.md#docker-socket-proxy)
- **Do:** Implement `marsa-dockerproxy`: listen on `docker.sock`, forward each connection to the guest's Docker API bridge over vsock, and pass through streaming bodies, chunked responses and connection upgrades without buffering whole bodies. Handle client disconnects and cancellation.
- **Deliver:** `crates/marsa-dockerproxy` and tests with a fake engine.
- **Accept when:**
  - `docker version`, `docker run -it`, `docker exec -it`, `docker logs -f`, `docker attach` and `docker build` work through the proxy on a real VM.
  - Memory use stays flat while streaming a multi-gigabyte `docker save`.
  - Closing the client closes the upstream connection.

### M4-02 Start the VM on demand

- **Status:** todo
- **Depends on:** M4-01, M3-06
- **Docs:** [03 VM lifecycle](../03-host-daemon.md#vm-lifecycle)
- **Do:** Make the proxy call `ensure_ready` before forwarding. Requests wait during boot up to a timeout and then return a clear error. Track running containers to drive the idle state.
- **Deliver:** proxy and supervisor integration.
- **Accept when:**
  - `docker ps` with the VM stopped starts it and succeeds.
  - Ten concurrent requests during boot all succeed from one boot.
  - With idle stop enabled and no containers, the VM stops after the timeout and the next request starts it again.

### M4-03 Fetch and bundle upstream binaries

- **Status:** done
- **Depends on:** M0-03
- **Docs:** [09 Bundled upstream binaries](../09-cli-and-docker-compat.md#bundled-upstream-binaries)
- **Do:** Implement `cargo xtask fetch-upstream` for the Docker CLI, Compose, Buildx and the macOS keychain credential helper (darwin/arm64), pinned by version and checksum in a versions file. Verify current release locations and layouts before pinning. Record each in the SBOM input.
- **Deliver:** the fetch step, `packaging/upstream-versions.toml`.
- **Accept when:**
  - A wrong checksum fails the step.
  - The binaries run and report the pinned versions.

### M4-04 Optional Docker socket symlink

- **Status:** blocked (owner decision Q-08)
- **Depends on:** M4-01
- **Docs:** [09 How clients find the engine](../09-cli-and-docker-compat.md#how-clients-find-the-engine), Q-08
- **Do:** Implement the opt-in `/var/run/docker.sock` symlink as a single, auditable administrator action with a matching removal.
- **Deliver:** the symlink helper.
- **Accept when:**
  - Creating and removing the symlink leaves no other change on the system.

Q-08 governs where the optional administrator action is offered. Context registration and environment discovery have their own task, M4-16.

### M4-05 Shared-folder allowlist

- **Status:** done
- **Depends on:** M0-04
- **Docs:** [07 The allowlist](../07-file-sharing-and-storage.md#the-allowlist), [11 Shared folders](../11-security.md#shared-folders), D-012
- **Do:** Implement `marsa-share`: the allowlist model, path canonicalisation with symlink resolution, the protected-path list as code, containment checks, and the `Shares` API service.
- **Deliver:** `crates/marsa-share` with thorough tests.
- **Accept when:**
  - Symlink, `..`, trailing-slash and case-variant escapes are refused (tested).
  - Protected paths cannot be added directly or through a parent.
  - The protected list cannot be changed through settings.

### M4-06 Attach shares and enforce bind-mount policy

- **Status:** todo
- **Depends on:** M4-05, M4-01, M3-04
- **Docs:** [07 Shared folders](../07-file-sharing-and-storage.md#shared-folders), [07 Correctness requirements](../07-file-sharing-and-storage.md#correctness-requirements), V-06
- **Do:** Map allowlist entries to virtiofs shares mounted at identical paths in the guest. Inspect `POST /containers/create` in the proxy for bind mounts outside the allowlist and return a clear error with the fix; raise an approval event that the GUI and CLI can answer (once, always, deny). Handle adding a share with or without a VM restart according to backend capabilities. Implement whatever M1-04 found necessary for change notifications and ownership.
- **Deliver:** share attachment, proxy inspection, approval event flow, a file-sharing compatibility test suite.
- **Accept when:**
  - `docker run -v <allowed path>:/x` works; an unlisted path is refused with an actionable message.
  - Files created in a container on a share are owned by the Mac user on the host.
  - A file edit on the host triggers a watcher inside a container (hot reload test with a common dev server).
  - V-06 is resolved in the docs.

### M4-07 Data disk management

- **Status:** todo
- **Depends on:** M3-03, M2-05
- **Docs:** [07 Data disk](../07-file-sharing-and-storage.md#data-disk)
- **Do:** Create the sparse data image on first run with the configured maximum size, grow it when the setting increases, expose usage through the `Disk` service, exclude it from Time Machine by default with a setting, and guarantee a clean guest shutdown on quit, logout and host shutdown.
- **Deliver:** data-disk module in `marsad`, `Disk.Usage`.
- **Accept when:**
  - A fresh install creates and formats the disk on first boot.
  - Increasing the size limit grows the filesystem at next boot.
  - Killing `marsad` mid-run and restarting recovers without data loss in a test that writes a volume first.

### M4-08 NAT networking, DNS and host names

- **Status:** todo
- **Depends on:** M3-04, M2-07
- **Docs:** [06 Network modes](../06-networking.md#network-modes), [06 DNS](../06-networking.md#dns), [06 Container to host](../06-networking.md#container-to-host), V-07
- **Do:** Verify the NAT attachment requirements (V-07). Implement reading the host resolver configuration and pushing DNS to the guest at boot and on network change, and the `host.docker.internal`, `gateway.docker.internal` and `host.marsa.internal` names.
- **Deliver:** network manager NAT path, the `Network.Status` call.
- **Accept when:**
  - A container can pull from a public registry and resolve names.
  - A container reaches a service listening on the Mac through `host.docker.internal`.
  - Switching Wi-Fi networks restores resolution without a restart.

### M4-09 Port forwarder

- **Status:** todo
- **Depends on:** M4-08, M2-08
- **Docs:** [06 Published ports](../06-networking.md#published-ports), V-08, Q-07
- **Do:** Implement `marsa-net`'s forwarder for TCP and UDP driven by agent port events: bind on loopback by default (LAN exposure behind a setting), IPv4 and IPv6 loopback, conflict detection with a clear error, teardown on close. Verify unprivileged binding of low ports on current macOS (V-08).
- **Deliver:** `crates/marsa-net` forwarder, `Network.ListForwardedPorts`.
- **Accept when:**
  - `docker run -p 8080:80 nginx` is reachable at `localhost:8080` over IPv4 and IPv6.
  - A UDP echo container works through a forwarded port.
  - A port already in use yields an error naming the port (and the owner when discoverable).
  - With LAN exposure off, the port is not reachable from another machine.
  - Stopping the container closes the listener.

### M4-10 User-space network mode and automatic selection

- **Status:** todo
- **Depends on:** M4-09, M3-15
- **Docs:** [06 The user-space stack](../06-networking.md#the-user-space-stack), [06 Automatic mode selection](../06-networking.md#automatic-mode-selection), V-07
- **Do:** Use the qualified transport from M3-14 and boot-time mode selection from M3-15. Implement `userspace` port forwarding and network-change recovery with events. Respect a pinned mode.
- **Deliver:** user-space mode, the probe, `Network.SetMode`.
- **Accept when:**
  - With NAT egress blocked by a firewall rule in a test, `auto` switches to `userspace` and a pull succeeds.
  - The switch is reported as an event with its reason.
  - A pinned mode is never overridden.
  - Test no lease, native DNS and TCP failure, filtered and split DNS, invalid TLS, helper denial or exit, and Wi-Fi or VPN changes. Keep the local engine available and preserve host policy.
  - The authenticated helper control path is private. Guest access to host loopback and metadata services follows explicit policy.
  - V-07 is resolved in the docs.

### M4-11 Proxy settings and CA certificate import

- **Status:** todo
- **Depends on:** M4-08
- **Docs:** [06 Proxies and certificates](../06-networking.md#proxies-and-certificates)
- **Do:** Read system proxy settings and manual overrides and push them to the guest. Export the certificates the user trusts from the macOS keychain and push them before the engine starts and on change. Add settings for registry mirrors and insecure registries.
- **Deliver:** proxy and certificate handling on the host side.
- **Accept when:**
  - Behind a test proxy with TLS interception, `docker pull` succeeds after the proxy's CA is trusted in the keychain.
  - Changing the proxy setting applies without reinstalling.

### M4-12 Registry credentials

- **Status:** todo
- **Depends on:** M4-03, M4-16
- **Docs:** [11 Registry credentials](../11-security.md#registry-credentials)
- **Do:** Offer to set the keychain credential helper as the Docker CLI credential store, never overwriting an existing store. Confirm that no credential is written to disk in plain text or stored in the guest.
- **Deliver:** credential-store setup logic and tests with config fixtures.
- **Accept when:**
  - `docker login` stores the credential in the keychain.
  - An existing `credsStore` or `credHelpers` entry is left untouched.

### M4-13 SSH agent forwarding

- **Status:** todo
- **Depends on:** M4-01
- **Docs:** [09 SSH agent forwarding](../09-cli-and-docker-compat.md#ssh-agent-forwarding)
- **Do:** Forward the host SSH agent socket to a fixed path in the guest over vsock, on demand.
- **Deliver:** forwarding in `marsad` and the agent.
- **Accept when:**
  - `docker build --ssh default` can clone a private repository in a test.
  - No key material is copied; only the agent protocol is forwarded.

### M4-14 Conformance suite (gate G4)

- **Status:** todo
- **Depends on:** M4-02, M4-06, M4-07, M4-09, M4-10, M4-11, M4-12
- **Docs:** [09 Compatibility checklist](../09-cli-and-docker-compat.md#compatibility-checklist), [13 System tests](../13-testing-and-benchmarks.md#system-tests-real-vm)
- **Do:** Build the automated conformance suite covering every line of the compatibility checklist with the real `docker` CLI and Compose, at least one Testcontainers suite per listed language, and a Dev Containers open-and-build check. Assert the documented intended differences return their explicit errors.
- **Deliver:** `tests/conformance/`, wired into Tier 2.
- **Accept when:**
  - The suite passes on the virtualization runner.
  - `--platform linux/amd64` fails with the documented message.
  - Results are published as a CI artifact per run.

### M4-15 Apply staged Docker engine configuration

- **Status:** todo
- **Depends on:** M2-07, M3-06, M4-08
- **Docs:** [06 Proxies and certificates](../06-networking.md#proxies-and-certificates), [D-027](../15-decisions.md#d-027-apply-proxy-settings-at-a-controlled-engine-restart)
- **Do:** Connect staged proxy, trust and gateway changes to a controlled engine restart. Validate the requested config, coordinate admission of new engine requests and preserve active workloads where Docker supports it. Require explicit user approval when the action can stop running workloads.
- **Deliver:** daemon and guest restart coordination with end-to-end tests.
- **Accept when:**
  - Valid staged proxy settings take effect for registry and BuildKit requests after the coordinated restart.
  - The host keeps desired settings when an update requires a later guest boot.
  - Active workloads are preserved, or the action reports the impact and requires explicit approval before stopping them.
  - A failed engine restart reports the failure and restores the last working configuration.

### M4-16 Docker context and endpoint discovery

- **Status:** in progress
- **Depends on:** M4-01, M4-03
- **Docs:** [09 How clients find the engine](../09-cli-and-docker-compat.md#how-clients-find-the-engine), V-12
- **Do:** Register the `marsa` context with the bundled upstream Docker CLI. Preserve current context selection and let users select Marsa explicitly. Expose the Docker socket and endpoint for `marsa env` through the authenticated host API. Verify common client discovery behavior.
- **Deliver:** typed backend context and endpoint service, isolated integration tests, and the client compatibility note.
- **Accept when:**
  - Explicit context registration is repeatable, preserves other contexts and the existing `config.json`, and refuses to overwrite an unrelated `marsa` context.
  - The bundled upstream CLI sees the registered context and the host API returns the correct `DOCKER_HOST` data.
  - V-12 is resolved in the docs.
