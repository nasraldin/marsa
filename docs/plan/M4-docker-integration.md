# M4 Docker integration

**Outcome:** the standard `docker` CLI, Compose, Buildx and common tools work against Marsa: socket, contexts, shared folders, data disk, networking and ports.

Design: [03 Docker socket proxy](../03-host-daemon.md#docker-socket-proxy), [06 Networking](../06-networking.md), [07 File sharing and storage](../07-file-sharing-and-storage.md), [09 CLI and Docker compatibility](../09-cli-and-docker-compat.md).

---

### M4-01 Docker socket proxy

- **Status:** done
- **Depends on:** M3-07, M2-08
- **Docs:** [03 Docker socket proxy](../03-host-daemon.md#docker-socket-proxy)
- **Do:** Implement `marsa-dockerproxy`: listen on `docker.sock`, forward each connection to the guest's Docker API bridge over vsock, and pass through streaming bodies, chunked responses and connection upgrades without buffering whole bodies. Handle client disconnects and cancellation.
- **Deliver:** `crates/marsa-dockerproxy` and tests with a fake engine.
- **Accept when:**
  - `docker version`, `docker run -it`, `docker exec -it`, `docker logs -f`, `docker attach` and `docker build` work through the proxy on a real VM.
  - Memory use stays flat while streaming a multi-gigabyte `docker save`.
  - Closing the client closes the upstream connection.

### M4-02 Start the VM on demand

- **Status:** done
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

- **Status:** done
- **Depends on:** M4-01
- **Docs:** [09 How clients find the engine](../09-cli-and-docker-compat.md#how-clients-find-the-engine), Q-08
- **Do:** Implement the opt-in `/var/run/docker.sock` symlink as a single, auditable administrator action with a matching removal.
- **Deliver:** the symlink helper.
- **Accept when:**
  - Creating and removing the symlink leaves no other change on the system.

[D-036](../15-decisions.md#d-036-offer-the-optional-docker-socket-link-in-settings) places the optional administrator action in Settings. Context registration and environment discovery have their own task, M4-16.

On November 25, 2026, the owner approved both native macOS 27.2.1
prompts in an isolated desktop session. The root-owned system link pointed
to the daemon's owner-only socket. Removal left the private socket in
place, restored the original absence of the system link, and matched the
pretest runtime directory listing. macOS fixture tests also verify that
another engine's symlink, an occupied directory and a dangling foreign
link stay in place. The Settings card was checked in English light,
English dark, Arabic light and Arabic dark.

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

- **Status:** done
- **Depends on:** M4-05, M4-01, M4-03, M3-16, M9-18
- **Docs:** [07 Shared folders](../07-file-sharing-and-storage.md#shared-folders), [07 Correctness requirements](../07-file-sharing-and-storage.md#correctness-requirements), V-06
- **Do:** Map allowlist entries to virtiofs shares at identical paths. Use the pinned Docker Engine authorization plugin for container-create checks and preserve the byte-transparent host proxy. Authorize each explicit bind source through the bounded VM policy channel. Expose owner approval actions for once, always and deny, with explicit restart on Apple VZ. Preserve the observed ownership behavior. M5-09 and M7-10 present the approvals. M4-19 qualifies the hot-reload workflow.
- **Deliver:** share attachment, Docker authorization, typed approval events and decisions, and signed file-sharing tests.
- **Accept when:**
  - `docker run -v <allowed path>:/x` works; an unlisted path is refused with an actionable message.
  - Files created in a container on a share are owned by the Mac user on the host.
  - One-session and persistent approvals take effect at restart. The session grant expires at the following stop and the persistent grant survives.
  - The production ownership behavior and the supported change-notification workflow are documented. M4-19 owns the dev-server acceptance and final V-06 disposition.


**Qualification.** The signed Apple silicon guest preserved raw HTTP 200 attach, HTTP 101 BuildKit upgrades and private SSH builds with the upstream authorization hook. The hook denied disallowed binds through fixed-length, chunked and native HTTP/2 REST, plus invalid or missing JSON. The product inspector caps container-create JSON at 1 MiB. The signed packaged tests verified Mac ownership, one-session expiry, persistent approval and the exact guest Docker socket mount. With authorization enabled, Docker preserved BuildKit, interactive terminals, log following and attach. A 2,215,736,832-byte Docker save caused zero observed daemon resident-memory growth over 405 samples.

### M4-19 Qualify host edits in a container development server

- **Status:** done
- **Depends on:** M4-06, M4-03
- **Docs:** [07 Correctness requirements](../07-file-sharing-and-storage.md#correctness-requirements), V-06
- **Do:** Test an upstream development server that relies on file-change events in the signed default Apple virtualization runtime. Use scoped Compose Watch sync to writable container storage. Record limitations for development servers using direct virtiofs binds.
- **Deliver:** repeatable Tier 2 development server fixture and user documentation.
- **Accept when:**
  - Editing host source triggers hot reload in a running container, including a common editor's atomic save.
  - The test confirms only selected development files reach the container.
  - V-06 is resolved in the docs.

**Qualification.** On macOS 27.2.1 and guest Linux 7.3.5, unmodified Docker Engine 29.10.0 and Compose 5.8.0 copied the initial Hugo content. Hugo sent a browser LiveReload message over WebSocket and served newly rendered HTML after a normal write and an atomic host rename. The container kept the same startup time. It had no mounted directories or published ports. The explicitly ignored file and both out-of-scope files stayed absent.

Tier 2 downloads the test-only, Apache-2.0 Linux ARM64 Hugo 0.170.1 release, verifies its pinned SHA-256 against the artifact, and imports it into the disposable Docker engine. Hugo runs in its native watch mode. The executable reports the upstream release revision `c8b03de9564cf846a3986e879727d638bb96ab33`. The fixture uses guest storage, a private Docker configuration, and a container-local socket. [Hugo release and checksums](https://github.com/gohugoio/hugo/releases/tag/v0.170.1) · [Hugo server](https://gohugo.io/commands/hugo_server/)

Run the test with the [packaged guest and helper](../../crates/marsad/tests/packaged_runtime.rs) configured and the verified [upstream Docker bundle](../09-cli-and-docker-compat.md#bundled-upstream-binaries) on `PATH`. Stage the Hugo archive before starting the signed run.

```sh
export MARSA_VZ_WATCH_HUGO_ARCHIVE="$(python3 crates/marsad/tests/packaged_runtime/compose_watch.py fetch target/dev-fixtures)"
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad --test packaged_runtime \
  compose_watch::packaged_compose_watch_reloads_hugo_after_host_saves \
  -- --exact --ignored --nocapture --test-threads=1
```

### M4-07 Data disk management

- **Status:** done
- **Depends on:** M3-03, M2-05, M3-12, M4-01
- **Docs:** [07 Data disk](../07-file-sharing-and-storage.md#data-disk)
- **Do:** Create the sparse data image on first run with the configured maximum size, grow it when the setting increases, expose usage through the `Disk` service, exclude it from Time Machine by default with a setting, and guarantee a clean guest shutdown on quit, logout and host shutdown.
- **Deliver:** data-disk module in `marsad`, `Disk.Usage`.
- **Accept when:**
  - A fresh install creates and formats the disk on first boot.
  - Increasing the size limit grows the filesystem at next boot.
  - Killing `marsad` mid-run and restarting recovers without data loss in a test that writes a volume first.

**Verification.** On macOS 27.2.1 the signed packaged daemon created a private 64 GiB sparse image and reported host and guest usage. The test verified Time Machine exclusion and opt-out. A separate 1 GiB VM wrote a named volume while Docker was active, then survived a host daemon `SIGKILL` and grew the same ext4 disk to 2 GiB on restart. A subsequent `SIGTERM` left ext4 clean without pending journal recovery. M4-01 provides the Docker socket for the physical recovery test.

### M4-08 NAT networking, DNS and host names

- **Status:** done
- **Depends on:** M3-04, M2-07
- **Docs:** [06 Network modes](../06-networking.md#network-modes), [06 DNS](../06-networking.md#dns), [06 Container to host](../06-networking.md#container-to-host), V-07
- **Do:** Verify the NAT attachment requirements (V-07). Implement reading the host resolver configuration and pushing DNS to the guest at boot and on network change, and the `host.docker.internal`, `gateway.docker.internal` and `host.marsa.internal` names.
- **Deliver:** native NAT DNS and automatic Docker host aliases. M4-18 owns the shared `Network.Status` call and verification of the user-space resolver.
- **Accept when:**
  - A container can pull from a public registry and resolve names.
  - A container reaches a service listening on the Mac through `host.docker.internal`.
  - Switching Wi-Fi networks restores resolution without a restart.

On November 26, 2026, an explicitly selected native NAT VM pulled BusyBox 1.37.0. The running container resolved the public registry before and after the operator switched the Mac to a second Wi-Fi network. The host changed IPv4 subnets. The container kept its ID and start time and reported zero restarts. The default bridge container had no compatibility host entry, and its DNS gateway returned NXDOMAIN for `host.docker.internal`. An HTTP request to the alias failed at name resolution. An independent request to a temporary HTTP service bound to the Mac’s native NAT gateway address also timed out. A repeat with the system TCP listener served the expected HTTP body by gateway IP. The subsequent fix supplies the native gateway aliases to both Docker bridge resolvers. The signed packaged regression pulled the pinned BusyBox image, verified the alias A record over TCP, resolved the public registry and fetched an HTTP response through each alias from both bridge types. A running container retained its ID and resolved all three aliases and the registry after a physical Wi-Fi handoff. Its host HTTP request succeeded after the handoff.

### M4-09 Port forwarder

- **Status:** done
- **Depends on:** M4-08, M2-08, M4-23
- **Docs:** [06 Published ports](../06-networking.md#published-ports), [D-038](../15-decisions.md#d-038-bind-published-ports-to-loopback-by-default)
- **Do:** Implement `marsa-net`'s forwarder for TCP and UDP driven by agent port events: bind on loopback by default (LAN exposure behind a setting), IPv4 and IPv6 loopback, conflict detection with a clear error, teardown on close. Report denied host binds with the affected address and port.
- **Deliver:** `crates/marsa-net` forwarder, `Network.ListForwardedPorts`.
- **Accept when:**
  - `docker run -p 8080:80 nginx` is reachable at `localhost:8080` over IPv4 and IPv6.
  - A UDP echo container works through a forwarded port.
  - A port already in use yields an error naming the port (and the owner when discoverable).
  - With LAN exposure off, the port is not reachable from another machine.
  - Stopping the container closes the listener.

On November 26, the signed packaged tests exercised IPv4 and IPv6 TCP and UDP
on native NAT and the user-space attachment. A separate macOS Tart client was
denied by default, received the workload response after LAN opt-in, and lost
access when LAN was disabled on the same publications. Explicit localhost
bindings stayed private throughout. The tests exercised concurrent datagram
flows, conflicting listeners, container removal and engine shutdown.
The authenticated port list returned the conflicting address, port and
discoverable owner. Keyboard interaction saved the setting in all four
[desktop variants](../screenshots/m4-09/). The separate peer run requires
a reachable Tart client and the Mac’s two peer-facing addresses. See the
[packaged test](../../crates/marsad/tests/packaged_runtime/forwarding.rs).

### M4-10 User-space network mode and automatic selection

- **Status:** in progress
- **Depends on:** M4-09, M3-15, M4-25
- **Docs:** [06 The user-space stack](../06-networking.md#the-user-space-stack), [06 Automatic mode selection](../06-networking.md#automatic-mode-selection), V-07
- **Do:** Use the qualified transport from M3-14 and boot-time mode selection from M3-15. Implement `userspace` port forwarding and network-change recovery with events. Respect a pinned mode.
- **Deliver:** user-space port forwarding and network-change recovery using the boot probe. M4-25 owns `Network.SetMode`.
- **Accept when:**
  - With NAT egress blocked by a firewall rule in a test, `auto` switches to `userspace` and a pull succeeds.
  - The switch is reported as an event with its reason.
  - A pinned mode is never overridden.
  - Test no lease, native DNS and TCP failure, filtered and split DNS, invalid TLS, helper denial or exit, and Wi-Fi or VPN changes. Keep the local engine available and preserve host policy.
  - The authenticated helper control path is private. Guest access to host loopback and metadata services follows explicit policy.
  - V-07 is resolved in the docs.

### M4-11 Proxy settings and CA certificate import

- **Status:** todo
- **Depends on:** M4-08, M4-21
- **Docs:** [06 Proxies and certificates](../06-networking.md#proxies-and-certificates)
- **Do:** Read system proxy settings and manual overrides and use the approved activation from M4-21. Export the certificates the user trusts from the macOS keychain and push them before the engine starts and on change. Add settings for registry mirrors and insecure registries.
- **Deliver:** proxy and certificate handling on the host side.
- **Accept when:**
  - Behind a test proxy with TLS interception, `docker pull` succeeds after the proxy's CA is trusted in the keychain.
  - Changing the proxy setting applies without reinstalling.

### M4-12 Registry credentials

- **Status:** done
- **Depends on:** M4-03, M4-16
- **Docs:** [11 Registry credentials](../11-security.md#registry-credentials)
- **Do:** Offer to set the keychain credential helper as the Docker CLI credential store, never overwriting an existing store. Confirm that no credential is written to disk in plain text or stored in the guest.
- **Deliver:** credential-store setup logic and tests with config fixtures.
- **Accept when:**
  - `docker login` stores the credential in the keychain.
  - An existing `credsStore` or `credHelpers` entry is left untouched.

**Verification.** A signed daemon fixture in a private Docker configuration applied the opt-in, preserved the selected context and refused to change a configured store. The pinned macOS Docker CLI and Keychain helper stored and retrieved one generated credential using a local Engine API fixture. The client configuration had no credential field, and the disposable Keychain item was removed and verified absent.

### M4-13 SSH agent forwarding for builds

- **Status:** done
- **Depends on:** M4-01
- **Docs:** [09 SSH agent forwarding](../09-cli-and-docker-compat.md#ssh-agent-forwarding), [11 SSH agent access](../11-security.md#ssh-agent-access)
- **Do:** Qualify the upstream Buildx SSH mount through the Docker socket proxy. Use Buildx's per-build session and the invoking client's explicitly supplied SSH agent. Keep the separate guest socket proposal in M4-17.
- **Deliver:** Signed VM integration test using a disposable SSH agent, test key and local private Git repository.
- **Accept when:**
  - `docker build --ssh default` can clone the private fixture repository.
  - The test key remains on the Mac. It signs the authentication on the host through the agent protocol.
  - A build without the SSH provider and a container launched without one have no agent access.

### M4-14 Conformance suite (gate G4)

- **Status:** todo
- **Depends on:** M4-02, M4-06, M4-07, M4-09, M4-10, M4-11, M4-12, M4-13, M4-19
- **Docs:** [09 Compatibility checklist](../09-cli-and-docker-compat.md#compatibility-checklist), [13 System tests](../13-testing-and-benchmarks.md#system-tests-real-vm)
- **Do:** Build the automated conformance suite covering every line of the compatibility checklist with the real `docker` CLI and Compose, at least one Testcontainers suite per listed language, and a Dev Containers open-and-build check. Assert the documented intended differences return their explicit errors.
- **Deliver:** `tests/conformance/`, wired into Tier 2.
- **Accept when:**
  - The suite passes on the virtualization runner.
  - `--platform linux/amd64` fails with the documented message.
  - Results are published as a CI artifact per run.

### M4-15 Apply staged Docker engine configuration

- **Status:** todo
- **Depends on:** M2-07, M3-06, M4-08, M4-11, M4-21
- **Docs:** [06 Proxies and certificates](../06-networking.md#proxies-and-certificates), [D-027](../15-decisions.md#d-027-apply-proxy-settings-at-a-controlled-engine-restart)
- **Do:** Extend controlled engine restart from M4-21 to the full proxy, trust and gateway policy, including M4-08 native networking and automatic Docker aliases. Validate the requested config, coordinate admission of new engine requests and preserve active workloads where Docker supports it. Require explicit user approval when the action can stop running workloads.
- **Deliver:** daemon and guest restart coordination with end-to-end tests.
- **Accept when:**
  - Valid staged proxy settings take effect for registry and BuildKit requests after the coordinated restart.
  - The host keeps desired settings when an update requires a later guest boot.
  - Active workloads are preserved, or the action reports the impact and requires explicit approval before stopping them.
  - A failed engine restart reports the failure and restores the last working configuration.

### M4-16 Docker context and endpoint discovery

- **Status:** done
- **Depends on:** M4-01, M4-03
- **Docs:** [09 How clients find the engine](../09-cli-and-docker-compat.md#how-clients-find-the-engine), V-12
- **Do:** Register the `marsa` context with the bundled upstream Docker CLI. Preserve current context selection and let users select Marsa explicitly. Expose the Docker socket and endpoint for `marsa env` through the authenticated host API. Verify common client discovery behavior.
- **Deliver:** typed backend context and endpoint service, isolated integration tests, and the client compatibility note.
- **Accept when:**
  - Explicit context registration is repeatable, preserves other contexts and the existing `config.json`, and refuses to overwrite an unrelated `marsa` context.
  - The bundled upstream CLI sees the registered context and the host API returns the correct `DOCKER_HOST` data.
  - V-12 is resolved in the docs.

### M4-17 Explicit SSH agent access for containers

- **Status:** blocked (stock Docker and VZ do not provide host-verifiable container identity)
- **Depends on:** M4-06, M4-13, M4-24
- **Docs:** [09 SSH agent forwarding](../09-cli-and-docker-compat.md#ssh-agent-forwarding), [11 SSH agent access](../11-security.md#ssh-agent-access)
- **Do:** Design explicit authorization for a named container to mount an SSH agent socket at a documented guest path. Enforce the grant on the host, restrict the approved agent operations, authenticate the guest transport, and revoke access on container exit, VM stop or user request. Qualify the VSock transport needed by that design.
- **Deliver:** Container-scoped SSH forwarding in the host daemon and guest agent, with a documented opt-in and revocation flow.
- **Accept when:**
  - An authorized container can use an explicitly chosen host agent with a disposable test key.
  - An unapproved container and a guest-initiated connection without a current grant cannot use the agent.
  - Revocation closes active streams and rejects subsequent connections. Host private key bytes stay on the host.

- **Blocker:** M4-24 proved Linux opener checks and stream teardown in a disposable VM. Stock Docker admits exec into the approved container through the supported guest Docker socket. The process then has the approved container's real Linux identity. The host's vsock carries VM-level provenance, so it cannot distinguish that request from one initiated by the approved workload. The acceptance criteria require a host-verifiable workload boundary or an owner-approved change to the trust model. [Identity and lifecycle review](../11-security.md#container-agent-grant-boundary)

### M4-18 Qualify fallback DNS recovery and expose network status

- **Status:** done
- **Depends on:** M2-07, M3-12, M3-14, M3-15, M5-01
- **Docs:** [06 The user-space stack](../06-networking.md#the-user-space-stack), [06 DNS](../06-networking.md#dns)
- **Do:** Verify live scoped resolver changes through the existing user-space DNS gateway. Expose the configured and active attachment, current guest IPv4 and gateway, and available connectivity reason through the authenticated typed API. Keep native NAT and Docker host aliases in M4-08.
- **Deliver:** `Network.Status`, typed client access, and a signed packaged-runtime DNS test.
- **Accept when:**
  - An authenticated caller can read the configured mode while the engine is stopped. While it is running, the result reports the active mode, available connectivity reason, and separately observed IPv4 and gateway. A stopped guest has no stale addresses.
  - A signed packaged user-space VM resolves a controlled macOS-scoped name from existing default-bridge and custom-network containers. Both containers observe a changed scoped answer without restart, and the Docker API remains available.
  - The existing registry TLS and host-security checks still pass. The live test uses disposable DNS records and leaves host networking and firewall settings alone.

- **Evidence:** On macOS 27.2.1, the signed packaged runtime returned the current user-space guest and gateway addresses through `Network.Status`. Existing default-bridge and custom-network containers saw the macOS local-only DNS answer change from `192.0.2.64` to `192.0.2.65` with unchanged startup timestamps. The signed registry pull with TLS verification and the helper's loopback-isolation fixture also passed.

With the pinned Docker CLI on `PATH` and the built guest and network-helper paths set for the [packaged runtime tests](../../crates/marsad/tests/packaged_runtime.rs), run:

```sh
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad --test packaged_runtime \
  fallback_dns::scoped_dns_updates_reach_existing_containers_without_restart \
  -- --exact --ignored --nocapture --test-threads=1
```

### M4-20 Provision approved manual daemon proxy at guest startup

- **Status:** done
- **Depends on:** M2-07, M3-06, M3-12, M4-18
- **Docs:** [06 Proxies and certificates](../06-networking.md#proxies-and-certificates), [D-027](../15-decisions.md#d-027-apply-proxy-settings-at-a-controlled-engine-restart)
- **Do:** Prepare a credential-free HTTP/HTTPS daemon proxy policy from private last-known-good state on the explicitly selected user-space attachment. Hand it to the negotiated control session before releasing Docker on a cold boot. A Settings.Update only edits the draft. The runtime preserves absent/default policy behavior and fails closed for an incompatible guest or attachment.
- **Deliver:** Packaged daemon boot provisioning and regression tests. M4-21 owns the approved activation action and writes its successful policy to the private state.
- **Accept when:**
  - Default startup and reconnect still work. A draft edit is durable and leaves both the live and next ordinary boot policy unchanged.
  - A packaged guest receives the validated policy before StartEngine. Rejected policy or unsupported startup deferral prevents the engine from being released.
  - A signed packaged-runtime fixture reads back the approved daemon proxy, including on a subsequent cold boot. This check uses no external service and makes no registry or BuildKit routing claim. M4-21 owns signed destination evidence.

- **Evidence:** On macOS 27.2.1, the signed packaged fixture passed in 51.20 seconds with Docker Engine 29.10.0. The running engine returned both approved settings from [its info endpoint](https://github.com/moby/moby/blob/docker-v29.10.0/api/types/system/info.go). A draft saved through the settings RPC left the running engine and its next cold boot on the same approved settings and data disk. The draft remained pending, and neither endpoint appeared in daemon or guest console logs. Protocol tests also cover rejected configuration and a guest without startup deferral.

With built guest and network-helper paths set for the [packaged runtime tests](../../crates/marsad/tests/packaged_runtime.rs), run:

```sh
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad --test packaged_runtime \
  daemon_proxy::approved_proxy_survives_cold_boots_and_draft_edits \
  -- --exact --ignored --nocapture --test-threads=1
```

### M4-21 Activate staged manual daemon proxy with approval and rollback

- **Status:** done
- **Depends on:** M3-17, M4-20, M9-18, M4-06
- **Docs:** [06 Proxies and certificates](../06-networking.md#proxies-and-certificates), [D-027](../15-decisions.md#d-027-apply-proxy-settings-at-a-controlled-engine-restart)
- **Do:** Expose an authenticated preview and explicit revision-bound activation for manual daemon proxy on the pinned user-space attachment. Coordinate engine admission and any approved workload interruption. Retain the previous working policy until the candidate succeeds, and restore it after a failed candidate. Persist successful activation separately from the desired draft.
- **Deliver:** Controlled activation, durable last-known-good state, and signed end-to-end tests using a controlled reachable proxy destination.
- **Accept when:**
  - A valid approved proxy carries actual registry and default Docker-driver BuildKit requests. The signed test confirms routing through the approved proxy with the bind-authorization service enabled.
  - Draft edits persist without interrupting workloads, and ordinary boot/retry never activates an unapproved draft.
  - The user sees the potential impact before granting approval. Stale approvals and racing starts or connects cannot apply or use a candidate out of order.
  - A failed candidate leaves the draft pending, reports failure, and restores the last working configuration on the same data disk. Cancellation and daemon recovery preserve the last working policy.

- **Evidence:** On macOS 27.2.1, the final signed packaged test passed in 88.36 seconds with Docker Engine 29.10.0, BuildKit 0.35.0 and Buildx 0.39.1. The running engine kept its original proxy during draft save and after a declined confirmation. Explicit approval retained the same data disk and wrote an owner-only policy record. Docker pull and the default Docker-driver BuildKit build used separate restricted CONNECT requests and obtained distinct registry manifests through verified TLS. The fixture exported the BuildKit base image marker. The test used guest loopback and a fixture-only CA. The [reproduction](../../crates/marsad/tests/packaged_runtime/proxy_fixture/README.md) includes guest preparation. Fake-backend tests cover stale and expired approvals, delayed-worker replay after disconnect, candidate rollback and uncommitted VM cleanup.

### M4-23 Verify unprivileged macOS loopback binds

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [06 Published ports](../06-networking.md#published-ports), V-08
- **Do:** Check standard-user IPv4 and IPv6 loopback binds below 1024 with a control at 1024, for both TCP and UDP. Record the result and the port forwarder’s expected behavior when macOS denies a bind.
- **Deliver:** verified port privilege behavior in the networking design and verification ledger.
- **Accept when:**
  - The documented experiment records the macOS version, address families, transports, and OS errors.
  - No host privilege, LAN listener, or system setting is required.

### M4-24 Qualify SSH agent transport and guest peer identity

- **Status:** done
- **Depends on:** M3-03, M3-16, M4-06, M4-13
- **Docs:** [11 SSH agent access](../11-security.md#ssh-agent-access), [D-030](../15-decisions.md#d-030-scope-build-ssh-agent-access-to-buildx-sessions), [RFC 9987](https://www.rfc-editor.org/rfc/rfc9987.html)
- **Do:** Qualify host-initiated VZ access to a closed, disposable guest relay and validate Unix opener identity against a pinned cgroup with the peer's Linux pidfd. Prove that filesystem visibility of the same socket is insufficient. Check that the forwarded agent accepts bounded identity listing and signing while rejecting mutations. Keep the test agent, key and VM separate from user state.
- **Deliver:** Entitled disposable fixture and a documented transport and identity boundary for M4-17. M4-17 owns production grant and immutable Docker-ID mapping, socket mount authorization, and lifecycle revocation.
- **Accept when:**
  - The signed host connects to the guest relay and the relay verifies the host CID. Neither an unsolicited guest-to-host connection nor a guest opener without the expected peer identity gains access.
  - Two guest Linux cgroups see the same Unix socket. Only the approved one can verify a signature made by its explicitly selected disposable Mac agent, and a forbidden mutation leaves the agent's key intact.
  - Closing the host transport disconnects an active guest stream and another request fails. The key and approval exist only inside the disposable fixture lifecycle. Document the privileged guest and Docker integration limits.

- **Evidence:** The entitled test completed in 1.03 seconds on macOS 27.2.1 with Linux 7.3.5 and OpenSSH 10.5p1. The host-opened VZ connection was authenticated as host CID 2. The unsolicited guest-to-host dial did not connect. A second cgroup opening the identical Unix socket received EOF before any SSH request. The approved opener verified a real Ed25519 signature, and its remove-all attempt left exactly one key in the dedicated host agent. Closing the host stream produced EOF on the active guest stream, refused a fresh open, and the peer pidfd signaled exit. [Fixture and reproduction](../../crates/marsa-vm-vz/tests/real_vm/ssh_fixture/README.md)

### M4-25 Save the configured network mode through the typed API

- **Status:** done
- **Depends on:** M3-15, M4-18, M4-21
- **Docs:** [03 Typed API](../03-host-daemon.md#typed-api), [06 Automatic mode selection](../06-networking.md#automatic-mode-selection)
- **Do:** Implement owner-authenticated `Network.SetMode` using the existing settings transaction and request deduplication. Change only the configured mode. Report that saved mode and any independently observed active attachment and guest addresses. Leave the current VM and its connections alone.
- **Deliver:** A typed settings mutation with disposable-state and fake-runtime tests.
- **Accept when:**
  - Only the authenticated owner can set `auto`, `nat`, or `userspace`. Invalid modes and request IDs change nothing. Repeating a request returns its original result, and using its ID for a different mode is rejected.
  - Only the network mode changes on disk and in settings. Concurrent edits serialize without losing unrelated settings. A stopped or unavailable runtime has no active mode or address reported.
  - Changing the mode while the VM is ready leaves its active attachment intact. An ordinary restart uses the saved explicit pin and automatic selection does not override it.

- **Evidence:** The authenticated Unix tests verified validation, duplicate and conflicting request IDs, preservation of unrelated settings and serialization with full settings updates. The daemon process wrote an owner-only file in a disposable home and read its saved mode after relaunch without booting a VM. The fake packaged runtime kept NAT active during a Userspace edit, then selected the new pin only on an ordinary restart. A failed boot left active fields empty.

### M4-26 Cache Docker inventory in the daemon

- **Status:** done
- **Depends on:** M3-08
- **Docs:** [03 Responsibilities](../03-host-daemon.md#responsibilities), [08 Data flow](../08-gui.md#data-flow), [D-034](../15-decisions.md#d-034-cache-docker-inventory-in-the-daemon)
- **Do:** Persist last-known Docker overview sections (containers, images, volumes, networks, version, disk) under the daemon state directory. Refresh them in the background while the VM is ready. Extend `Stats.DockerOverview` with `prefer_cache` and `observed_at_unix_millis` (host API minor 14). Live section reads update the cache; container stats stay live-only. Use an owner-only JSON file until SQLite fits the dependency graph (D-034).
- **Deliver:** `marsad` inventory cache module, proto and PROTOCOL notes, client support for cached reads, unit tests with a disposable state directory.
- **Accept when:**
  - A second `prefer_cache` read returns the stored JSON and timestamp without contacting Docker when the cache is warm.
  - A live read refreshes the store; restarting against the same state directory still serves the last snapshot.
  - Background refresh runs only while the engine is ready or idle and stops with the daemon.

- **Evidence:** Unit tests round-trip the cache file across reopen. The typed-api overview fixture fills the cache with a live read, stops the guest Docker script, and still returns the same JSON and timestamp with `prefer_cache`.
