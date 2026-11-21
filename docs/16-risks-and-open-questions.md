# 16 Risks and open questions

## Open questions for the project owner

These need a decision from a person. Agents must not decide them.

| # | Question | Recommendation | Blocks |
|---|---|---|---|
| Q-01 | Which open-source licence? | Apache-2.0, matching the Docker engine, gpui-component and libkrun | M0-02 (adding `LICENSE`), first public release |
| Q-03 | Is the name "Marsa" clear to use? Trademark search in the software class, domain, GitHub organisation, crates.io, Homebrew names | Check before any public release | M10 |
| Q-04 | Who holds the Apple Developer Program account and the update signing key? | A project-owned account; the update key offline or hardware-backed | M10 |
| Q-06 | Who reviews Arabic translations and terminology? | A fluent reviewer plus a term glossary | M6, M7 |
| Q-07 | Default for LAN exposure of published ports: loopback only (proposed) or Docker Desktop's behaviour? | Loopback only | M4 |
| Q-08 | Should `/var/run/docker.sock` be offered during onboarding or only in settings? | Settings only, with a hint when a tool fails to connect | M4, M7 |
| Q-09 | Business model and governance (contributions, CLA or DCO, security contact)? | DCO, a `SECURITY.md` contact | First public release |
| Q-10 | Is any telemetry wanted at all? | None by default (decided); decide whether an opt-in exists | M9 |

## Resolved platform and runner questions

| # | Finding |
|---|---|
| Q-02 | The owner selected macOS 27 as the minimum supported version on 2026-11-21. |
| Q-05 | The owner selected their current Apple silicon Mac as the planned Tier 2 host on 2026-11-21. Registration is pending. See the [runner setup](12-build-release-update.md#tier-2-runner-setup). |
| V-13 | GitHub-hosted Apple silicon runners do not support nested virtualization. Tier 2 requires separate hardware. Verified on 2026-11-21 against [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#limitations-for-arm64-macos-runners). |

## Technical unknowns to verify

Each is labelled **Verify** where it appears in the design docs. The owning task must resolve it and update the doc.

| # | Unknown | Where | Resolved by |
|---|---|---|---|
| V-01 | Does GPUI meet the right-to-left criteria on macOS? | [08](08-gui.md) | M1-01 |
| V-04 | How much guest memory does the balloon actually return to macOS? | [04](04-virtualization-macos.md) | M9-04 |
| V-05 | Does guest discard reach the sparse image so APFS frees blocks? | [07](07-file-sharing-and-storage.md) | M1-03, M9-03 |
| V-06 | virtiofs behaviour: change notifications, ownership mapping | [07](07-file-sharing-and-storage.md) | M4-06 |
| V-07 | NAT attachment requirements; user-space attachment class | [06](06-networking.md) | M4-08, M4-10 |
| V-08 | Binding privileged ports without root on current macOS | [06](06-networking.md) | M4-09 |
| V-09 | libkrun: current release, API, kernel requirements (own kernel or libkrunfw) | [04](04-virtualization-macos.md) | M11-02 |
| V-10 | gpui-component's i18n and whether Fluent can coexist with it | [08](08-gui.md) | M6-04 |
| V-11 | State of GPUI accessibility on macOS | [08](08-gui.md) | M6-08 |
| V-12 | Which tools ignore Docker contexts | [09](09-cli-and-docker-compat.md) | M4-04 |
| V-14 | Login-item and agent registration API on the minimum macOS | [03](03-host-daemon.md) | M3-09 |
| V-15 | Networking packages the pinned engine needs in the guest | [05](05-guest-image.md) | M2-03 |
| V-16 | Licences of the fonts to bundle | [08](08-gui.md) | M6-02 |
| V-17 | Root filesystem format support (EROFS versus SquashFS) in the chosen kernel and tools | [05](05-guest-image.md) | M2-04 |

### Verified findings

| # | Finding | Evidence |
|---|---|---|
| V-02 | Both bridges booted the same stock Linux fixture and delivered start, vsock and shutdown callbacks. M1-02 recommends the Rust bindings for gate G2. | [M1-02](spikes/M1-02-vz-binding.md) |
| V-03 | All planned VZ device and lifecycle classes are available by macOS 12. The full configuration booted on macOS 27.2.1 with a 27.0 deployment target and the virtualization entitlement. | [Availability and boot results](spikes/M1-02-vz-binding.md#verified-class-availability) |

## Risk register

| # | Risk | Likelihood | Impact | Mitigation | Owner milestone |
|---|---|---|---|---|---|
| R-01 | GPUI cannot deliver correct Arabic and right-to-left | Medium to high | High: GUI framework change | Spike first (M1-01); Tauri fallback defined (D-005); direction built into the component layer either way | M1 |
| R-02 | GPUI API churn breaks the build on upgrade | High | Medium | Pin GPUI and gpui-component revisions; upgrade in dedicated tasks; keep Marsa's wrapper layer thin and central | M6 |
| R-03 | Memory is not returned to macOS under `vz` | Medium | Medium | Idle stop as the dependable mechanism; balloon as a bonus; `krun` backend as the long-term answer | M9, M11 |
| R-04 | File sharing is too slow or semantically off for real projects | Medium | High | Benchmarks and a compatibility suite early (M4); guidance for named volumes; `krun` evaluation | M4, M11 |
| R-05 | Corporate networks break pulls and builds | High | High | Automatic user-space fallback; proxy and CA import; test matrix | M4, M9 |
| R-06 | A container-escape CVE lands and the fix is slow | Medium | High | Whole-image updates; monitoring; a rehearsed fast-release path | M10 |
| R-07 | Scope is too large for the team | High | High | macOS only; strict "must have" list; Kubernetes and assistant deferred; milestones gated by spikes | All |
| R-08 | No suitable CI hardware for VM tests | Medium | High | Fake backend keeps most tests on hosted runners; plan a self-hosted Mac (Q-05) | M0 |
| R-09 | Signing, notarization or entitlements delay the release | Medium | Medium | Do a signed "hello VM" build early (M3-10), not at the end | M3, M10 |
| R-10 | Tool compatibility gaps (Testcontainers, Dev Containers) | Medium | High | Conformance suite with real tools from M4 | M4 |
| R-11 | Agent misuse through the MCP server causes data loss | Low to medium | High | Tiers, approvals, audit; volumes always need approval | M8 |
| R-12 | Name or trademark conflict | Unknown | Medium | Q-03 before any public release | M10 |
| R-13 | Accessibility falls short on GPUI | Medium | Medium | Verify early (V-11); keyboard operation is fully in Marsa's control | M6 |
| R-14 | Disk space is not reclaimed, repeating Docker Desktop's most common complaint | Medium | High | Prove discard end to end in the spike (M1-03) | M1, M9 |
| R-15 | Numbers quoted from other projects do not hold for Marsa | High | Low | Treat them as targets; publish only Marsa's own measurements | M11 |

## Assumptions

- A small team, mostly AI coding agents directed by the project owner, builds Marsa.
- The owner can provide Apple developer credentials, a signing setup and at least one Apple silicon Mac for testing.
- Upstream Docker static binaries for linux/arm64 and the darwin/arm64 CLI remain available under their current licences.
