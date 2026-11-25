# M7 GUI features

**Outcome:** every "must have" screen, built and tested in four variants.

Design: [08 GUI](../08-gui.md), feature list in [08 Feature priorities](../08-gui.md#feature-priorities).

Every task here carries these standing acceptance conditions in addition to its own:

- Snapshots committed in English light, English dark, Arabic light and Arabic dark.
- All strings present in both locales.
- Fully operable by keyboard; every action registered in the command palette.
- Loading, empty, error and engine-stopped states implemented.
- Technical text isolated as left-to-right.

---

### M7-01 Containers list

- **Status:** in progress
- **Depends on:** M6-09, M4-02
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Virtualised container list grouped by Compose project with standalone containers in their own group; columns for name, image, state, ports and age; start, stop, restart and remove (with confirmation) per row and in bulk; filter and search; live updates from engine events.
- **Deliver:** screen and tests.
- **Accept when:**
  - 2,000 containers scroll smoothly and state changes appear within one second.
  - Remove always asks for confirmation; running containers require an explicit force choice.

The desktop route renders grouped live container snapshots. Detected Lima rows use the same table and keep their source badge. Start and stop target the authenticated Unix socket captured for that row. Passive invalidation without an idle lease is pending.

---

### M7-02 Container detail and inspect

- **Status:** in progress
- **Depends on:** M7-01
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Detail view with tabs: overview, logs, terminal, inspect, stats. Inspect shows environment, ports, mounts, networks, labels, and the raw JSON with copy. Environment values that look like secrets are masked until revealed.
- **Deliver:** screen and tests.
- **Accept when:**
  - Every inspect section matches `docker inspect`.
  - Masked values are not exposed in snapshots or logs.

The detail route opens detected Lima containers through the same authenticated socket as the selected row. The detail route masks environment fields. The clipboard export contains redacted inspect metadata.

---

### M7-03 Logs

- **Status:** in progress
- **Depends on:** M7-02, M6-07
- **Docs:** [08 Performance rules](../08-gui.md#performance-rules)
- **Do:** Log viewer with follow, pause, search with match navigation, level and text filters, timestamps toggle, wrap toggle, ANSI colour rendering, copy and save. A combined view for several containers or a whole Compose project with a colour and label per source. Bounded ring buffer with a "load earlier" action.
- **Deliver:** log view, performance test.
- **Accept when:**
  - At 10,000 lines per second for an hour, frame times stay within budget and memory does not grow after warm-up.
  - Search runs off the UI thread and stays responsive on a full buffer.
  - Mixed Arabic and English log lines render in correct order within a left-to-right log line.

Bounded per-container and combined streams are wired. External log streams use the selected engine and keep Compose log sources within that engine. High-rate, navigation, and visual acceptance remain open.

---

### M7-04 Terminal

- **Status:** in progress
- **Depends on:** M7-02
- **Docs:** [08 Performance rules](../08-gui.md#performance-rules)
- **Do:** Interactive terminal into a container through the exec API: shell detection with a fallback list, resize, copy and paste, bounded scrollback, multiple sessions. Verify the terminal component available for the pinned GPUI ecosystem before building.
- **Deliver:** terminal view and tests.
- **Accept when:**
  - Full-screen programs (an editor, `top`) render and resize correctly.
  - Closing the tab ends the exec session.

The Terminal view uses bounded Docker exec sessions and the pinned vt100
renderer. It includes IME input, viewport resizing, scrollback, copy and paste.
Both locked dependency audits accepted the renderer. Interactive terminal and
four-variant acceptance remain open.

---

### M7-05 Stats

- **Status:** in progress
- **Depends on:** M7-02
- **Docs:** [08 Performance rules](../08-gui.md#performance-rules)
- **Do:** Live CPU, memory, network and disk I/O per container with bounded history charts, plus an overview across running containers and the VM totals from the daemon.
- **Deliver:** stats views and tests.
- **Accept when:**
  - Values match `docker stats` within sampling error.
  - Charts keep time flowing left to right in both languages, with labels localised.

The container view presents sampled CPU, memory, network and disk metrics. Overview reads passive VM totals. A cross-container roll-up is pending.

---

### M7-06 Images

- **Status:** in progress
- **Depends on:** M6-09, M4-02
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Image list with repository, tag, size, age and in-use state; pull by reference with progress; remove with confirmation; run from image with a simple form; image detail with metadata. Pulling an amd64-only image shows the documented explanation.
- **Deliver:** screens and tests.
- **Accept when:**
  - Pull progress is shown per layer and can be cancelled.
  - In-use images cannot be removed without an explicit force choice.

The desktop route wires metadata, local runs, cancellable ARM64 pulls and guarded removal. Private-registry pulls still use the CLI.

---

### M7-07 Volumes and networks

- **Status:** in progress
- **Depends on:** M6-09, M4-02
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Lists with usage (which containers), size for volumes, create and remove. Volume removal uses the typed-confirmation dialog and names the containers that used it.
- **Deliver:** screens and tests.
- **Accept when:**
  - A volume in use cannot be removed.
  - Volume removal always requires typed confirmation.

The volume and network routes wire typed mutations. Volume removal requires the exact name and rejects an attached resource.

---

### M7-08 Compose projects

- **Status:** in progress
- **Depends on:** M7-01, M7-03
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Project list detected from container labels; project detail with services, their state, combined logs; up, down (volumes kept unless explicitly chosen) and restart, executed through the bundled Compose with streamed output.
- **Deliver:** screens and tests.
- **Accept when:**
  - Actions produce the same result as the equivalent `docker compose` command.
  - "Down with volumes" requires typed confirmation.

The desktop route uses an explicitly selected local Compose file, bounded output, and typed confirmation for volume removal.

---

### M7-09 Engine control and resources

- **Status:** in progress
- **Depends on:** M6-09, M3-08, M9-13
- **Docs:** [03 Settings](../03-host-daemon.md#settings)
- **Do:** Engine status view with start, stop and restart, boot progress by stage, failure display with console tail and a "collect diagnostics" action. Resource settings for CPUs, memory, disk limit, idle stop and backend, stating clearly when a restart is needed.
- **Deliver:** screens and tests.
- **Accept when:**
  - A simulated boot failure shows the failing stage and log tail.
  - Changing resources applies at the next start and says so.

The desktop route wires lifecycle controls, resource preferences, boot details and diagnostics through the daemon.

---

### M7-10 Settings

- **Status:** in progress
- **Depends on:** M6-09, M4-05, M4-06, M4-10, M4-11
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Settings sections: General (language, theme, start at login), Resources, Sharing (allowlist management with protected-path explanations), Network (mode, LAN exposure, proxy, registries, DNS), Docker (context, optional socket symlink, CLI links, credential store), Updates, About (versions of every component, licences).
- **Deliver:** screens and tests.
- **Accept when:**
  - Every setting in the daemon's settings model is reachable.
  - Language and theme changes apply live.
  - About lists the versions from the guest manifest and the upstream binaries.
  - Sharing shows exact pending bind paths and offers once, always and deny. An approved static share displays the restart requirement and preserves workloads until the owner explicitly applies it.

General settings includes the Figma accent choices, desktop appearance, language,
launch at login, and notification preferences. Network settings use guarded
proxy activation. LAN exposure (Q-07), the global Docker socket (Q-08), and
unsupported host-tool integration remain open.

Docker settings show the external socket and offer a rescan. A detected Unix
engine's containers, images, volumes, networks and Compose projects appear
on their respective resource pages with source badges. External inventory
never enters the Marsa engine's mutation or selection paths.

---

### M7-11 Disk and cleanup

- **Status:** blocked (M5-12 exact Docker deletion)
- **Depends on:** M6-09, M5-05, M5-08, M5-12, M9-03
- **Docs:** [07 Disk cleanup with a preview](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
- **Do:** Disk usage breakdown by category and a cleanup flow that previews exactly what will be removed and how much space returns to the Mac, with volumes unselected by default, then runs it. Show a separate explicit reclaim action.
- **Deliver:** screens and tests.
- **Accept when:**
  - The preview equals what is removed.
  - After cleanup and explicit reclaim, report the observed host allocation change. M9-14 connects automatic reclaim.

The desktop route exposes storage usage, candidate identities, the engine's
apply capability and explicit reclaim. M7-21 routes the Images cleanup
shortcut to that review and previews external environments without a
destructive apply. Resume apply when M5-12 can bind eligibility and identity
to the deletion itself.

---

### M7-12 Activity and agent approvals

- **Status:** in progress
- **Depends on:** M6-09, M8-02, M8-03
- **Docs:** [10 Approval flow](../10-mcp-server.md#approval-flow), [10 Audit log](../10-mcp-server.md#audit-log)
- **Do:** Activity screen showing the daemon event history and the agent audit log with filters. Approval prompts for agent requests and bind-mount requests showing full detail (what, which client, what will be affected), with approve, deny and, where allowed, "always for this project". Grant management with revoke.
- **Deliver:** screens and tests.
- **Accept when:**
  - A destroy-tier request shows exactly what will be removed, including volume names and sizes.
  - "Always" is not offered for destroy or host-affecting tiers.
  - Approvals can be answered from a notification when the window is closed.

Activity shows engine events, authenticated approvals, grant management and
filtered audit records. Notifications can reopen the approval screen after the
last window closes. Approval decisions stay in the authenticated review UI.

---

### M7-13 Notifications and global states

- **Status:** in progress
- **Depends on:** M6-09
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** System notifications for events that need attention (engine failure, update available, approval pending, network mode switched), a consistent error surface with actionable messages, and global offline and engine-stopped states.
- **Deliver:** notification module and tests.
- **Accept when:**
  - Each notification type can be turned off in settings.
  - Every error message says what happened and what the user can do.

The desktop shell presents in-window error states and opt-in macOS notifications.
The app-owned daemon subscription continues while all windows are closed and the
app is still running. Notification actions reopen the relevant review screen.

---

### M7-14 Arabic review and four-variant audit (gate G5)

- **Status:** todo
- **Depends on:** M7-01, M7-02, M7-03, M7-04, M7-05, M7-06, M7-07, M7-08, M7-09, M7-10, M7-11, M7-12, M7-13, Q-06
- **Docs:** [08 Four variants, always](../08-gui.md#four-variants-always), D-015
- **Do:** Audit every screen in all four variants against a checklist: mirroring, icon direction, technical-text isolation, truncation and overflow with long Arabic strings, digit style, contrast, focus order. Have a fluent Arabic reviewer check all strings for correctness and consistent terminology against the glossary. Fix everything found.
- **Deliver:** `docs/reference/four-variant-audit.md` with the checklist and results, corrected strings and layouts.
- **Accept when:**
  - Every screen passes every checklist item in all four variants.
  - The reviewer signs off the Arabic strings and glossary.

---

### M7-15 Manage containers on detected Docker engines

- **Status:** in progress
- **Depends on:** M7-01's existing source-tagged table and M7-02's container detail route
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Route terminal, stats, and lifecycle requests through the selected row's pinned endpoint. Add expand and collapse controls to the grouped Containers views.
- **Deliver:** Desktop controls and endpoint regression coverage.
- **Accept when:**
  - An external terminal uses the selected socket for create, upgrade, and resize.
  - Live stats use the selected socket and the existing bounded charts.
  - Group controls work across the visible Marsa and external Compose groups.

On this Mac, the signed app opened an interactive shell and streamed live usage on Lima with the Marsa engine stopped. Expand and collapse worked on the selected group. The later context qualification included the four appearance and language variants.

### M7-16 Manage resources on detected Docker engines

- **Status:** in progress
- **Depends on:** M7-15
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Carry the selected endpoint through resource rows, dialogs, transfers, and Compose operations. Offer explicit engine selection for creation and pulls. Enable Docker actions according to the resource state and the selected engine's capabilities.
- **Deliver:** Engine-scoped resource actions in the existing Images, Volumes, Networks, and Compose views.
- **Accept when:**
  - Resource IDs and names shared by two engines resolve only on the selected one.
  - Confirmations identify the engine before removing data.
  - Compose execution uses trusted local project configuration and an authenticated connection.

Verified on Lima that the image tag resolves to the original image and volume creation uses the engine chosen in the dialog. M7-17 adds external Compose deployment and teardown from a deliberately selected local file.


### M7-17 Capability and context coverage

- **Status:** done
- **Depends on:** M7-16
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Add configured contexts, authenticated remote transports, and the remaining resource actions across engines. Keep context selection visible in project, registry, and destructive workflows.
- **Deliver:** Context list, capability-driven action availability and workflows for image transfer, network attachment, and Compose deployment from trusted files.
- **Accept when:**
  - The desktop can show multiple configured engines simultaneously and scopes operations to the selected engine even when object identifiers coincide.
  - Authenticated remote transports use the selected context's identity and security settings.
  - Registry push, image import and export, and network attachment are available where the chosen engine supports them.

Qualification used two isolated Unix test engines and an offline registered context, SSH with positive and negative host-trust checks, and a mutual-TLS proxy to Lima Docker Engine 29.9.2. The signed app deployed a disposable stack through the TLS context. The Lima fixture round-tripped an image archive, attached and detached a network, and pushed a test image to a loopback-only registry.


### M7-18 Context navigation and desktop feedback

- **Status:** done
- **Depends on:** M7-17 context inventory
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Filter all inventory views by context. Open the context list from the compact sidebar status card. Present transient alerts as toasts, keep persistent faults actionable, use semantic icon colors, and make full container rows respond to hover. Keep movement brief and respect Reduce Motion.
- **Deliver:** Desktop shell and inventory controls in English and Arabic.
- **Accept when:**
  - All four appearance and language combinations retain the current layout and expose context controls.
  - Switching context matches the row badges, details, and action destinations.
  - Toasts remain keyboard accessible and the sidebar status goes directly to the context list.

The signed desktop was reviewed in English light, English dark, Arabic light, and Arabic dark using isolated preferences. The context card opened Docker settings, the context filter isolated the selected source, and the deployment surfaced a dismissible toast.

### M7-19 Active Docker environment

- **Status:** done
- **Depends on:** M7-18
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Replace page-level context filters with an app-wide selection in the sidebar. Persist the selection and resolve external engines by both name and address. Keep each action pinned to the endpoint used to read the resource. Keep Marsa VM controls scoped to Marsa.
- **Deliver:** Environment switcher, scoped resource lists and commands, and an external overview with resource counts and connection status.
- **Accept when:**
  - Switching between stopped Marsa and reachable Lima changes the overview and resource action destinations. Both appear in the sidebar menu in English light, English dark, Arabic light, and Arabic dark.
  - The selected Lima environment survives relaunch. An unavailable selection does not route an operation to Marsa.
  - Search results and resource details carry the source endpoint. Existing per-row source badges remain visible.

The signed desktop switched between stopped Marsa and reachable Lima, displayed a Lima destination on the volume form, and restored Lima after relaunch. The sidebar menu was checked in both languages and appearances. The repository check and all 35 targeted tests passed.

### M7-20 Manage Docker environments

- **Status:** done
- **Depends on:** M7-19
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Add, edit, and test Marsa-managed Docker endpoints. Show verified engine identity and connection status. Keep manual connection settings distinct from discovered Docker CLI contexts. Give Settings grouped navigation with a compact layout for narrow windows.
- **Deliver:** Environment settings with input validation, connection testing, and confirmation before removing saved configurations.
- **Accept when:**
  - Unix, SSH, and mutual TLS endpoints can be configured and tested with the existing endpoint trust requirements.
  - The engine reconnects with saved settings after relaunch. Editing or removing an environment does not retarget an in-flight action.

Verified the engine identity fields against the [Moby 29.10.0 Engine API schema](https://github.com/moby/moby/blob/docker-v29.10.0/api/swagger.yaml). Saved settings retain a transport configuration and paths to TLS credential files. The files are read under the existing endpoint trust rules when connecting. The signed GUI tested, saved, renamed, selected, and restored a live Lima connection after relaunch. Environment settings were checked in both supported locales and appearances. Tests cover pinned targets, removal, and concurrent writes. A loopback mutual-TLS fixture verified both peers and rejected an invalid hostname and a broadly readable client key. The SSH checks covered transport configuration and untrusted-host rejection. A live connection to a trusted SSH server was unavailable during qualification.

### M7-21 Observe and maintain each environment

- **Status:** done
- **Depends on:** M7-20
- **Docs:** [08 Screens](../08-gui.md#screens), [13 Testing](../13-testing-and-benchmarks.md)
- **Do:** Surface container health, engine metrics, disk accounting, and events for the selected environment. Provide per-environment image update checks, cleanup review preferences, activity collection and retained-history lookback, health notifications, and inventory reports.
- **Deliver:** Environment dashboard metrics, activity, and maintenance controls.
- **Accept when:**
  - The dashboard and activity views are fed by the selected engine. They identify unavailable measurements and can recover from a disconnect.
  - A maintenance request targets the confirmed engine, previews destructive effects, and reports its outcome.

Managed activity uses the passive owner API, and external observations
stay pinned to the selected connection while they load or retry. Overview
loads live measurements after its cached first frame. The external disk
review shows available usage, candidate identities and the selected
endpoint. The [Moby 29.10.0 Engine API schema](https://github.com/moby/moby/blob/docker-v29.10.0/api/swagger.yaml)
documents the finite Activity query and verbose disk usage categories.
The signed Apple silicon test observed a real Docker container event,
previewed exact disposable storage identities and verified the resources
remained. Two isolated external engines verified Activity switching,
unavailable measurements and automatic recovery. Images opens the
selected engine's cleanup review, and the fixture logged zero mutation
requests. The native keyboard check opened Disk with Command–6 and closed
the review with Escape. The review was checked in
[English light](../spikes/M7-21-environment-observation/en-light.png),
[English dark](../spikes/M7-21-environment-observation/en-dark.png),
[Arabic light](../spikes/M7-21-environment-observation/ar-light.png), and
[Arabic dark](../spikes/M7-21-environment-observation/ar-dark.png).

Per-environment settings control Activity, retained history, registry checks,
unused volume visibility, and opt-in local health alerts for the selected
engine while the window is open. Docker keeps the in-memory event history.
Two external fixtures kept distinct settings after restart. Alpha requested
one hour of history and hid unused volume candidates while retaining volume
accounting. Beta issued no event requests after activity collection was
disabled. Tests cover alert event validation and deduplication. The settings
were verified in
[English light](../spikes/M7-21-environment-observation/settings-en-light.png),
[English dark](../spikes/M7-21-environment-observation/settings-en-dark.png),
[Arabic light](../spikes/M7-21-environment-observation/settings-ar-light.png), and
[Arabic dark](../spikes/M7-21-environment-observation/settings-ar-dark.png).

Cleanup apply reports that exact deletion is unsupported. M5-12 and M7-11
own the conditional deletion requirement. Verified Marsa release updates
use the app-wide updater.

### M7-22 Configure deploy-time secret providers

- **Status:** done
- **Depends on:** M7-20
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Manage supported deploy-time secret providers.
- **Deliver:** Settings connected to Compose deployment.
- **Accept when:**
  - Deployment resolves secrets without exposing stored credentials in logs or inspection output.

**Upstream behavior.** Compose 5.8.0 loads environment variables before it
interpolates a project. It reads an environment-backed runtime secret from
the finished project's environment and copies its bytes through the Docker
archive API into the container. A file-backed secret uses a bind mount.
The ordinary Compose invocation cannot receive a saved credential as a
regular interpolation variable.
[Pinned secret implementation](https://github.com/docker/compose/blob/v5.8.0/pkg/compose/secrets.go)
· [Pinned Compose loader](https://github.com/docker/compose/blob/v5.8.0/cmd/compose/compose.go).

**Qualification.** Pinned Keychain helper 0.9.13 stored, read, rotated and
removed a synthetic credential in an isolated namespace. Compose 5.8.0 and
Docker CLI 29.10.0 exercised the protected route against a disposable project
on Lima Docker Engine 29.9.2. The granted container received the right file,
the other container received none, and neither inspection, logs, interpolation,
nor the unchanged project file exposed the credential. Rotation recreated
only the granted container. A local image declaring a volume was rejected
before Keychain resolution.

Native GPUI captures on macOS show the Settings entry, the empty masked
editor and the deployment option in all four variants. Tab and Space enabled
the option, and Escape dismissed the editor in each variant.
[English light](../screenshots/m7-22/en-light.png) ·
[English dark](../screenshots/m7-22/en-dark.png) ·
[Arabic light](../screenshots/m7-22/ar-light.png) ·
[Arabic dark](../screenshots/m7-22/ar-dark.png).

### M7-23 Configure scanning, jobs, and downloads

- **Status:** todo
- **Depends on:** M7-21
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Add image scanner settings and cache controls, system-job configuration, and volume file downloads.
- **Deliver:** Persisted settings consumed by the corresponding workflows.
- **Accept when:**
  - Scanner settings affect a scan, maintenance schedules affect subsequent runs, and scan retention and cache cleanup report their outcomes.
  - Volume file downloads use the saved download format when the selected environment supports secure read-only access.

### M7-24 Compose template and new stack creation

- **Status:** blocked (signed desktop visual qualification pending)
- **Depends on:** M7-19
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Let users save or restore the default Compose template in Settings. Use the template when creating a new stack in an explicitly chosen project directory.
- **Deliver:** A persistent template editor, a new-stack creation flow, and deployment using the selected Docker environment.
- **Accept when:**
  - A saved template survives relaunch. Restoring the built-in template has an explicit action.
  - A new stack opens with the saved template. Creating a Compose file keeps any existing file intact and deployment uses the environment selected when the flow began.

The signed Compose client created the reviewed file in a disposable directory, deployed to the selected Lima socket, and removed the test stack afterward. Unit tests cover refusing existing files and symlinks. The Mac locked before the final four-variant desktop checks.

### M7-25 Desktop log preferences

- **Status:** blocked (signed desktop visual qualification pending)
- **Depends on:** M7-19
- **Docs:** [08 Screens](../08-gui.md#screens), [03 Settings](../03-host-daemon.md#settings)
- **Do:** Configure live log buffer size and time zone through the revisioned desktop settings. Retain bounded engine timestamps for the latest container and Compose output.
- **Deliver:** Localized log preferences and their effects in container and Compose logs.
- **Accept when:**
  - Buffer size and UTC or local time preferences survive relaunch and apply without an engine restart.
  - Container logs and combined Compose logs use the selected limits and time zone, without increasing existing resource bounds.
  - Log lines without usable engine timestamps remain readable.

Settings and viewer tests cover persisted choices, bounded buffers, and missing timestamps. The same Docker log records were checked in Dubai and New York, including the daylight saving transition rules. The Mac locked before the final four-variant desktop checks.

### M7-26 Show the installed release and its dependencies

- **Status:** done
- **Depends on:** M10-12
- **Docs:** [08 Screens](../08-gui.md#screens), [12 App bundle layout](../12-build-release-update.md#app-bundle-layout)
- **Do:** Extend the existing About screen with bilingual notes for the installed build, bundled Docker tool versions, live daemon and guest versions, and Finder access to bundled inventories and notices.
- **Deliver:** An offline-capable About screen and release notes included by bundle assembly.
- **Accept when:**
  - App version and bundled tools come from the installed build. Daemon and guest component versions come from the owner daemon when connected.
  - The correct release notes appear in each locale. Source checkouts and bundles missing an asset report its absence, and distribution inventory and notice actions reveal the installed evidence.

An offscreen native run used an ad hoc-signed copy of the assembled app that
passed strict, deep signature verification. The displayed app version matched
the bundle version. Both localized notes and four tool records matched their
installed files. The packaged owner daemon reported its version and 11 guest
component versions matching its bundled manifest with the engine stopped.

Tab and Space activated the four evidence actions. The fixture recorded each
macOS Finder selection call and verified its exact installed file path. The test
used the staged Docker inventory and local fixtures at the reserved app
inventory, guest inventory and notice paths. [M10-08](M10-release.md#m10-08-licence-notices-sbom-and-source-publication)
tracks the complete distribution evidence.

Native captures cover English and Arabic in light and dark appearances.
After the owner daemon disconnected, About kept its installed notes and
tools and showed the daemon connection state. A signed bundle with its
notes and tool inventory removed and an unbundled source executable both
showed asset absence.

### M7-27 Configure desktop registry credentials

- **Status:** blocked (credentialed image transfers require a pinned HTTPS path)
- **Depends on:** M7-20
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Provide registry settings for Docker Hub and private HTTPS registries. Use the signed bundled Docker credential helper and preserve existing CLI state. Bind credentials to the selected registry and only delegate transfers where the engine preserves that trust boundary.
- **Deliver:** Localized registry settings, owner-private credential persistence, real connection testing, and credentialed transfers with verified registry trust.
- **Accept when:**
  - Docker Hub is available by default. Owners can add, test, replace, and remove private registry credentials without weakening existing transport or authentication controls.
  - Pulls and pushes use credentials only for their intended registry, and unsupported engine or registry combinations fail without disclosing the credential.

The desktop settings support owner-private Keychain storage and direct HTTPS
connection checks. Credential delegation through the stock engine remains
blocked by its handling of registry-selected realms and redirects. See
[Registry credentials](../11-security.md#registry-credentials) for the pinned
source evidence and allowed connection paths.

### M7-28 Save and check out HTTPS Git repositories

- **Status:** blocked (signed desktop visual qualification pending)
- **Depends on:** M7-20
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md#git-sources)
- **Do:** Manage named HTTPS Git repository and branch references, test branch access, and create a checkout in a chosen folder for explicit selection by the Compose workflow.
- **Deliver:** An owner-private repository list and checkout through the operating system's trusted Git client.
- **Accept when:**
  - Saved repository entries survive restart and can be edited and removed.
  - Test verifies the branch exists. Checkout uses the selected reference and does not overwrite existing files.
  - Git ignores ambient configuration. The desktop keeps checkout and engine-pinned Compose deployment as distinct owner actions.

The client tested a real HTTPS branch, rejected a missing ref, and completed separate checkouts into explicitly chosen folders. Authenticated HTTPS and SSH are tracked in M7-30. The Mac locked before the final four-variant desktop checks.

### M7-29 Reusable container configuration sets

- **Status:** blocked (signed desktop visual qualification pending)
- **Depends on:** M7-20
- **Docs:** [08 Screens](../08-gui.md#screens), [03 Settings](../03-host-daemon.md#settings), [11 Security](../11-security.md)
- **Do:** Save named sets of container network, restart, environment, label, port and mount options. Let users apply and edit a copy before creating a container.
- **Deliver:** Localized configuration-set management and a typed create request targeted at the selected engine.
- **Accept when:**
  - Sets can be added, edited and removed with confirmation. Saved sets survive relaunch.
  - The create form shows the saved values and accepts overrides without changing the set. Docker receives the reviewed options for the selected engine.
  - Local binds remain subject to Marsa's existing allowlist. An external bind requires confirmation of the destination engine, source path and access mode.

A disposable Lima container verified options including environment, labels, restart policy, TCP and UDP ports, and read-only mounts. The same fixture verified the requirement for explicit external bind review. The Mac locked before the final four-variant desktop checks.

### M7-30 Authenticate Git sources

- **Status:** todo
- **Depends on:** M7-28
- **Docs:** [11 Security](../11-security.md)
- **Do:** Add HTTPS username and password or token credentials in macOS Keychain and SSH using verified host keys and an explicit private key reference.
- **Deliver:** A Git credentials view and authenticated branch test and checkout.
- **Accept when:** Tests and checkout use only the credential associated with the selected repository. No secret is written to settings or sent to Compose.

### M7-31 Download container files

- **Status:** blocked (signed desktop visual qualification pending)
- **Depends on:** M7-19
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Download an explicitly named path from a container to an owner-selected destination. Offer a persistent TAR or single-file download preference.
- **Deliver:** A container file download action and a format control under Settings.
- **Accept when:**
  - Downloads use the selected container and its pinned environment. A single file can be saved directly, while directories use TAR.
  - Truncated transfers and existing destinations are preserved. Marsa does not unpack archive paths into the host filesystem.

A disposable Lima container returned a binary file directly and a directory as TAR. The download refused to replace the existing destination. The pinned [Moby 29.10.0 schema](https://github.com/moby/moby/blob/docker-v29.10.0/api/swagger.yaml) documents the archive routes and path metadata header. The Mac locked before the final four-variant desktop checks.

### M7-32 Connect to Hawser agents

- **Status:** done
- **Depends on:** M7-20
- **Docs:** [08 Screens](../08-gui.md#screens), [11 Security](../11-security.md)
- **Do:** Implement Hawser Standard connections to a trusted agent and Hawser Edge connections through an authenticated reverse tunnel. Keep agent credentials in the platform credential store.
- **Deliver:** Two tested agent connection types in the environment editor, wired to engine selection and resource views.
- **Accept when:**
  - Real Standard and Edge agents pass connection testing and can be selected as the active engine.
  - Agent identity and authorization are checked before any Docker API request. Tests cover credential rejection, disconnect, and endpoint replacement while a request is in flight.

Verified both transports and terminal I/O against [Hawser upstream bca2770](https://github.com/Finsys/hawser/tree/bca2770d3c013f0559ac162372bca6ba75dfeed5) with private TLS fixtures. The tests reject incorrect tokens and verify that a disconnected Edge session cannot retarget an existing Docker request after reconnecting.

### M7-33 Refine desktop screen hierarchy and typography controls

- **Status:** done
- **Depends on:** M7-20
- **Docs:** [08 GUI](../08-gui.md)
- **Do:** Review the desktop routes and refine task grouping, control placement, and readable states. Group desktop preferences and provide native font and size pickers.
- **Accept when:** The typography controls persist and apply real fonts and sizes. Major route actions and states are discoverable, and scrollable content remains reachable in both supported languages and appearances.

On macOS, Settings applied and saved a different interface font, text size,
and terminal font. All three choices reappeared after restarting the owner
daemon and the desktop. The native text test verifies all six interface
sizes, bundled Latin and Arabic faces, Raleway's regular, medium, and
semibold weights, and the System and JetBrains Mono terminal faces.

The Extra Large checks used a disposable owner and a read-only engine fixture.
Keyboard shortcuts opened Containers, Disk & cleanup, and Settings in all
four variants. The checks reached later rows in Containers, Images, Volumes,
Networks, and Stacks while the table headings and filters stayed visible.
The Settings rail adapts to the font size and wraps longer labels. See the
Settings snapshots in [English light](../spikes/M7-33-typography/en-light.png),
[English dark](../spikes/M7-33-typography/en-dark.png),
[Arabic light](../spikes/M7-33-typography/ar-light.png), and
[Arabic dark](../spikes/M7-33-typography/ar-dark.png).

### M7-35 Restore desktop scrolling and finish the usability controls

- **Status:** done
- **Depends on:** M7-19, M7-20
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Repair the workspace scroll areas affected by PR #116 and finish its Overview and sidebar controls.
- **Accept when:**
  - Container, image, volume, network, and stack rows scroll within the remaining viewport. The page title, table headings, and filters stay visible.
  - Resource details can scroll independently. Selecting the active sidebar route returns to its list.
  - Overview exposes the container filters, shows measured history for top containers, and puts the local engine lifecycle controls in the active environment card.
  - Verify overflow and layout in English light, English dark, Arabic light, and Arabic dark.

Native macOS QA reproduced the stuck Images rows on the merged PR and verified
body scrolling in the rebuilt app with English light, English dark, Arabic
light, and Arabic dark. The checks reached later rows in all five workspaces
and the logs at the bottom of a 24-service stack detail. They also covered the
Containers keyboard shortcut, sidebar return to list, live healthy and
unhealthy filters, and the Overview stop confirmation.

### M7-36 Show the selected Lima VM on Machine

- **Status:** done
- **Depends on:** M7-19, M7-20
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Present the active owner-managed Lima instance on Machine with its resource allocation, supported network settings, and lifecycle controls.
- **Accept when:**
  - Machine shows the selected Lima instance and its CPU, memory, disk capacity, and supported loopback setting even when Docker is stopped.
  - Start, graceful stop, restart, and stopped-only settings edits use the exact verified Lima instance. Stop and restart require confirmation.
  - Switching away cannot send a pending confirmation or resource edit to the newly selected environment. Other providers keep their existing connection summary.
  - Check the Machine page in English light, English dark, Arabic light, and Arabic dark.

Verified the native Machine page in all four language and theme combinations. The
disposable Lima integration test covered graceful stop, allocation changes,
startup, and restart. The Machine page started the same VM and named it in
the restart confirmation.

### M7-37 Unify Machine and Overview lifecycle controls

- **Status:** done
- **Depends on:** M7-36
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Use one Machine layout for the selected environment and expose its available lifecycle controls in the Overview card.
- **Accept when:**
  - Marsa, owner-managed Lima, and connection-only environments use the same Machine identity, resources, and diagnostics layout.
  - Overview shows icon-only Start or Stop for the selected controllable VM, including an offline, stopped Lima instance.
  - Resource edits and lifecycle confirmations stay bound to the selected provider. Connection-only environments show verified information where available.
  - Check English and Arabic in light and dark mode.

Native checks covered four locale and theme combinations, Overview start and
confirmed stop, and saving a new CPU allocation on the disposable Lima VM.

### M7-38 Match Settings appearance to first-run setup

- **Status:** done
- **Depends on:** M7-35
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Present the interface language in a dropdown and share visual theme choices between Settings and first-run setup.
- **Accept when:**
  - The menu offers System, English (US), and العربية and reports the selected language.
  - Both surfaces use System, Light, and Dark previews with recognizable icons and a selected state.
  - Verify the four locale and theme combinations and the keyboard path.

Native Settings checks covered the four appearance and language combinations
and keyboard selection from the language menu.

### M7-39 Mirror Arabic vertical scrollbars

- **Status:** done
- **Depends on:** M7-35
- **Docs:** [08 Direction layer](../08-gui.md#direction-layer)
- **Do:** Place the shared vertical scrollbar and editor overview marks at the inline end of the viewport.
- **Accept when:**
  - Track paint, hit testing, and dragging mirror with the interface direction.
  - An area with both axes keeps the track corner clear and retains the existing horizontal scroll direction.
  - Verify English and Arabic in both appearances and cover a live direction change with a regression test.

Native checks dragged the Arabic Settings track and reached the final row of
a 38-volume table with its header fixed. Headless tests cover mirrored paint,
hit testing, dragging, and the two-axis corner.

### M7-40 Chart the Overview status, usage, and disk cards

- **Status:** done
- **Depends on:** M7-37
- **Docs:** [08 GUI](../08-gui.md)
- **Do:** Keep top running containers. Lay out Overview as four wrapping header cards—context (with status strip), CPU/memory history (percent plus absolute size against host budget), disk usage (donut and legend), and resources—then the top consumers list.
- **Deliver:** Overview chart layout, engine-level usage history, English and Arabic copy.
- **Accept when:**
  - The four header cards wrap together; context includes clickable lifecycle/health counts and the healthy banner.
  - CPU/memory show percent and consumed size against the host budget; disk uses a donut and legend; history updates from Overview samples without changing top consumers.
  - Verify English/Arabic and light/dark.

Headless coverage for engine history and the Overview chart layout is in place.
English and Arabic copy cover the history card, disk card, resources label, and healthy banner.

### M7-41 Paint Overview from the daemon inventory cache

- **Status:** done
- **Depends on:** M4-26, M7-05
- **Docs:** [08 Screens](../08-gui.md#screens), [08 Data flow](../08-gui.md#data-flow), [D-034](../15-decisions.md#d-034-cache-docker-inventory-in-the-daemon)
- **Do:** On Overview open, load Marsa inventory sections with `prefer_cache` so the first frame uses the daemon snapshot. Follow with a live refresh for utilization and disk. Do not add a GUI-side database.
- **Deliver:** Overview client cache-first reads and desktop first-paint behaviour.
- **Accept when:**
  - With a warm daemon cache, Overview shows container and resource counts before a live Docker round-trip completes.
  - Live stats and disk still update after the cached frame.
  - External engines keep their existing direct path (no GUI SQLite).

- **Evidence:** `OverviewClient::sample_cached` / `disk_cached` use host API minor 14. The desktop Overview loop paints from the cache when present, then immediately refreshes live utilization and disk.

### M7-42 Polish first-run setup selection, sharing, and RTL traffic lights

- **Status:** done
- **Depends on:** M7-38, M7-39
- **Docs:** [08 Screens](../08-gui.md#screens), [08 Direction layer](../08-gui.md#direction-layer)
- **Do:** Keep native traffic-light hover glyphs readable in Arabic by spanning the titlebar and forcing LTR on the controls. Use a green circle-check for selected language and appearance choices. Restyle the sharing step as a dashed drop zone with browse and folder cards.
- **Deliver:** Setup UI polish, English and Arabic copy, GPUI traffic-light container fix.
- **Accept when:**
  - Hovering close, minimize, and zoom in Arabic shows the same glyphs as English.
  - Selected language and appearance rows show a green circle-check.
  - Sharing offers a dashed drop/browse zone and lists chosen folders as removable cards.
  - Verify English/Arabic and light/dark.

### M7-43 Shared table shell on Images

- **Status:** done
- **Depends on:** M7-06, M7-42
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Add reusable table chrome (filter/sort/columns dropdowns, sortable headers, multi-select) and apply it on Images: one row per image; columns Image, Tags, Size, Age, State, Actions; usage filter All / In use / Some unused / Unused with icons; Tags count + in-use tag badges; Run / Push / Remove row actions. Layers open from the details modal (M7-46).
- **Deliver:** `screens/table` module, Images wiring, English and Arabic copy, unit tests for filter/sort/columns helpers.
- **Accept when:**
  - Images shows the default columns and supports header-click sort plus the Sort by / Order menu.
  - Usage filter and Columns picker (show/hide, Reset) work; Image column stays pinned.
  - Multi-select supports bulk remove with confirmation; row actions run, push, and remove; row click opens image details.
  - Verify English/Arabic and light/dark.

### M7-44 Environment brand marks and Images toolbar

- **Status:** done
- **Depends on:** M7-43
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Use the Marsa harbor mark for the Marsa environment entry; resolve Lima/Colima and other contexts via engine brand marks (Docker, Podman, …). Replace Images trailing actions with icon buttons for refresh, pull, load-from-tar, and a prune menu (unused vs dangling) backed by Docker `/images/prune`.
- **Deliver:** Sidebar icon fix, `prune_images` client route, Images icon rail, English and Arabic copy.
- **Accept when:**
  - Environment menu shows harbor for Marsa and the correct engine logo for Lima·docker / Podman-style sources.
  - Images toolbar offers refresh, pull, load tar, and both prune confirms.
  - Verify English/Arabic and light/dark.

### M7-45 Pull image wizard and Images select-all header

- **Status:** done
- **Depends on:** M7-44
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Replace the single-field Pull form with a Configure → Pull dialog matching the product flow: registry, image name, full reference preview, live layer table, progress, output log, then Keep / Remove. Move Images select-all into the table header column aligned with row checkboxes.
- **Deliver:** `resources/pull` wizard, Images header checkbox, English and Arabic copy, unit tests for reference composition and layer status.
- **Accept when:**
  - Pull dialog walks Configure → Pulling → completed Keep/Remove against a real engine stream (no demo data).
  - Select-all checkbox sits in the Images table header, not the toolbar.
  - Verify English/Arabic and light/dark.

### M7-46 Image details modal with layers stack

- **Status:** done
- **Depends on:** M7-45
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Opening an Images row shows a wide details modal titled like the layers screenshot (`Image layers: sha256:…`) with the expandable layer stack (size bars, opcode colours, Normal/Large, expand for Created/Size/Command/Comment). Keep run/push/remove on the parent table; also expose run/push/export/tag/remove as icons in the modal. Create-container from Run uses a single scrollbar. Parent columns use flexible widths.
- **Deliver:** `resources/image_detail`, `resources/layers`, `resources/run`, English and Arabic copy.
- **Accept when:**
  - Parent table keeps run/push/remove; clicking a row opens the details/layers modal.
  - Modal shows the layer stack matching the product screenshot and the five action icons.
  - Create container from Run has one scrollbar; columns fit Size/Age tightly.
  - Verify English/Arabic and light/dark.

### M7-47 Preserve desktop refinements after integration

- **Status:** done
- **Depends on:** M7-35, M7-37, M7-40
- **Docs:** [08 Screens](../08-gui.md#screens), [08 Direction layer](../08-gui.md#direction-layer)
- **Do:** Reconcile the final Containers toolbar and inactive Overview power control with the newer desktop changes on main.
- **Accept when:**
  - The view switches remain available when a filter has no results and expose the selected view to accessibility clients. The single group toggle and mirrored scrollbar stay functional.
  - Counting rows and sorting collapsed groups retain the cleanup from PR #123 without altering the new Updates filter.
  - The inactive Overview uses an accessible icon-only power action for the built-in engine or the selected managed Lima VM.
  - Check English and Arabic in both appearances.

### M7-48 Drive Overview inventory from Docker invalidations

- **Status:** done
- **Depends on:** M6-07, M7-41
- **Docs:** [08 Data flow](../08-gui.md#data-flow), [D-034](../15-decisions.md#d-034-cache-docker-inventory-in-the-daemon)
- **Do:** Bind the managed Overview resource counts to the normalized store. Keep live utilization and disk measurements independent of inventory refresh. Recompute built-in image indicators after container or image changes and preserve external registry checks.
- **Deliver:** Managed Overview selectors and tests separating collection invalidation from live measurements.
- **Accept when:**
  - Docker changes update the managed Overview counts without a periodic Docker inventory query.
  - Utilization history, disk measurements, and update indicators continue to refresh.
  - The cached first frame and external engine behavior from M7-41 are preserved.

**Qualification.** The service fixture held the initial live request open
until the cache had reached the store. An inventory invalidation triggered
one new read. A socket fixture verified that two subsequent utilization
samples requested only per-container stats. The managed projection retained
live measurements when the inventory counts changed, and the same socket
fixture covered the external dashboard's direct inventory and disk paths.
The existing built-in owner API supports local tag comparisons. The external
API path also compares registry digests through Docker's distribution route.
M7-52 tracks passive registry lookups for the built-in engine.

Native GPUI snapshots show a cached volume count of 1 and its update to 2
after replacing the normalized store snapshot. The fixture uses synthetic
inventory and keeps the engine disconnected.
[English light](../screenshots/m7-48/en-light.png) ·
[English dark](../screenshots/m7-48/en-dark.png) ·
[Arabic light](../screenshots/m7-48/ar-light.png) ·
[Arabic dark](../screenshots/m7-48/ar-dark.png).

### M7-49 Open Images details with the keyboard

- **Status:** done
- **Depends on:** M7-46
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Give the Images table and list a focusable control that opens the existing image details modal. The current row opens details from a pointer click.
- **Deliver:** A localized details control and a keyboard regression test.
- **Accept when:**
  - Tab reaches a localized details control in both views. Return and Space open the focused image's details.
  - Closing the modal restores focus to its trigger. Row selection, pointer opening, and Run, Push, and Remove remain independently operable.
  - Verify English and Arabic in light and dark mode.

Hidden macOS windows verified Tab, Return, Space and focus restoration after
Escape in the Images table and list. The fixture verified pointer opening from
the row and the details button, independent selection and Run, Push and Remove
dialogs, and access with Actions hidden. The fixture recorded only read
requests. Native snapshots show the focused control and opened modal in
[English light](../spikes/M7-49-image-details-keyboard/en-light.png),
[English dark](../spikes/M7-49-image-details-keyboard/en-dark.png),
[Arabic light](../spikes/M7-49-image-details-keyboard/ar-light.png), and
[Arabic dark](../spikes/M7-49-image-details-keyboard/ar-dark.png).

### M7-52 Check registry image digests for the built-in engine

- **Status:** in progress
- **Depends on:** M7-48
- **Docs:** [08 Screens](../08-gui.md#screens), [D-034](../15-decisions.md#d-034-cache-docker-inventory-in-the-daemon)
- **Do:** Define owner authorization for managed registry traffic. Add a typed, bounded lookup through the passive owner API for an already-running Docker engine and compare the results against the current normalized inventory on an independent schedule.
- **Deliver:** Managed registry digest checks, localized Settings text reflecting their authorization, and a request-path regression test.
- **Accept when:**
  - Registry lookups require the agreed owner authorization. Existing settings with image checks enabled by default do not silently authorize managed registry traffic.
  - A registry check uses validated image references, starts no VM, makes no inventory collection requests, and applies neither stale results nor results from a different engine.
  - Tests cover an authorized distribution request and absent requests when authorization is off or the engine is stopped.
