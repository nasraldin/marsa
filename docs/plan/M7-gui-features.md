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

- **Status:** todo
- **Depends on:** M6-09, M4-02
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Virtualised container list grouped by Compose project with standalone containers in their own group; columns for name, image, state, ports and age; start, stop, restart and remove (with confirmation) per row and in bulk; filter and search; live updates from engine events.
- **Deliver:** screen and tests.
- **Accept when:**
  - 2,000 containers scroll smoothly and state changes appear within one second.
  - Remove always asks for confirmation; running containers require an explicit force choice.

### M7-02 Container detail and inspect

- **Status:** todo
- **Depends on:** M7-01
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Detail view with tabs: overview, logs, terminal, inspect, stats. Inspect shows environment, ports, mounts, networks, labels, and the raw JSON with copy. Environment values that look like secrets are masked until revealed.
- **Deliver:** screen and tests.
- **Accept when:**
  - Every inspect section matches `docker inspect`.
  - Masked values are not exposed in snapshots or logs.

### M7-03 Logs

- **Status:** todo
- **Depends on:** M7-02, M6-07
- **Docs:** [08 Performance rules](../08-gui.md#performance-rules)
- **Do:** Log viewer with follow, pause, search with match navigation, level and text filters, timestamps toggle, wrap toggle, ANSI colour rendering, copy and save. A combined view for several containers or a whole Compose project with a colour and label per source. Bounded ring buffer with a "load earlier" action.
- **Deliver:** log view, performance test.
- **Accept when:**
  - At 10,000 lines per second for an hour, frame times stay within budget and memory does not grow after warm-up.
  - Search runs off the UI thread and stays responsive on a full buffer.
  - Mixed Arabic and English log lines render in correct order within a left-to-right log line.

### M7-04 Terminal

- **Status:** todo
- **Depends on:** M7-02
- **Docs:** [08 Performance rules](../08-gui.md#performance-rules)
- **Do:** Interactive terminal into a container through the exec API: shell detection with a fallback list, resize, copy and paste, bounded scrollback, multiple sessions. Verify the terminal component available for the pinned GPUI ecosystem before building.
- **Deliver:** terminal view and tests.
- **Accept when:**
  - Full-screen programs (an editor, `top`) render and resize correctly.
  - Closing the tab ends the exec session.

### M7-05 Stats

- **Status:** todo
- **Depends on:** M7-02
- **Docs:** [08 Performance rules](../08-gui.md#performance-rules)
- **Do:** Live CPU, memory, network and disk I/O per container with bounded history charts, plus an overview across running containers and the VM totals from the daemon.
- **Deliver:** stats views and tests.
- **Accept when:**
  - Values match `docker stats` within sampling error.
  - Charts keep time flowing left to right in both languages, with labels localised.

### M7-06 Images

- **Status:** todo
- **Depends on:** M6-09, M4-02
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Image list with repository, tag, size, age and in-use state; pull by reference with progress; remove with confirmation; run from image with a simple form; image detail with metadata. Pulling an amd64-only image shows the documented explanation.
- **Deliver:** screens and tests.
- **Accept when:**
  - Pull progress is shown per layer and can be cancelled.
  - In-use images cannot be removed without an explicit force choice.

### M7-07 Volumes and networks

- **Status:** todo
- **Depends on:** M6-09, M4-02
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Lists with usage (which containers), size for volumes, create and remove. Volume removal uses the typed-confirmation dialog and names the containers that used it.
- **Deliver:** screens and tests.
- **Accept when:**
  - A volume in use cannot be removed.
  - Volume removal always requires typed confirmation.

### M7-08 Compose projects

- **Status:** todo
- **Depends on:** M7-01, M7-03
- **Docs:** [08 Feature priorities](../08-gui.md#must-have-version-1)
- **Do:** Project list detected from container labels; project detail with services, their state, combined logs; up, down (volumes kept unless explicitly chosen) and restart, executed through the bundled Compose with streamed output.
- **Deliver:** screens and tests.
- **Accept when:**
  - Actions produce the same result as the equivalent `docker compose` command.
  - "Down with volumes" requires typed confirmation.

### M7-09 Engine control and resources

- **Status:** todo
- **Depends on:** M6-09, M3-08, M9-13
- **Docs:** [03 Settings](../03-host-daemon.md#settings)
- **Do:** Engine status view with start, stop and restart, boot progress by stage, failure display with console tail and a "collect diagnostics" action. Resource settings for CPUs, memory, disk limit, idle stop and backend, stating clearly when a restart is needed.
- **Deliver:** screens and tests.
- **Accept when:**
  - A simulated boot failure shows the failing stage and log tail.
  - Changing resources applies at the next start and says so.

### M7-10 Settings

- **Status:** todo
- **Depends on:** M6-09, M4-05, M4-06, M4-10, M4-11
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Settings sections: General (language, theme, start at login), Resources, Sharing (allowlist management with protected-path explanations), Network (mode, LAN exposure, proxy, registries, DNS), Docker (context, optional socket symlink, CLI links, credential store), Updates, About (versions of every component, licences).
- **Deliver:** screens and tests.
- **Accept when:**
  - Every setting in the daemon's settings model is reachable.
  - Language and theme changes apply live.
  - About lists the versions from the guest manifest and the upstream binaries.
  - Sharing shows exact pending bind paths and offers once, always and deny. An approved static share displays the restart requirement and preserves workloads until the owner explicitly applies it.

### M7-11 Disk and cleanup

- **Status:** todo
- **Depends on:** M6-09, M5-05
- **Docs:** [07 Disk cleanup with a preview](../07-file-sharing-and-storage.md#disk-cleanup-with-a-preview)
- **Do:** Disk usage breakdown by category and a cleanup flow that previews exactly what will be removed and how much space returns to the Mac, with volumes unselected by default, then runs it and triggers reclaim.
- **Deliver:** screens and tests.
- **Accept when:**
  - The preview equals what is removed.
  - After cleanup the reported host disk use drops accordingly (verified once M9-03 is done).

### M7-12 Activity and agent approvals

- **Status:** todo
- **Depends on:** M6-09, M8-02, M8-03
- **Docs:** [10 Approval flow](../10-mcp-server.md#approval-flow), [10 Audit log](../10-mcp-server.md#audit-log)
- **Do:** Activity screen showing the daemon event history and the agent audit log with filters. Approval prompts for agent requests and bind-mount requests showing full detail (what, which client, what will be affected), with approve, deny and, where allowed, "always for this project". Grant management with revoke.
- **Deliver:** screens and tests.
- **Accept when:**
  - A destroy-tier request shows exactly what will be removed, including volume names and sizes.
  - "Always" is not offered for destroy or host-affecting tiers.
  - Approvals can be answered from a notification when the window is closed.

### M7-13 Notifications and global states

- **Status:** todo
- **Depends on:** M6-09
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** System notifications for events that need attention (engine failure, update available, approval pending, network mode switched), a consistent error surface with actionable messages, and global offline and engine-stopped states.
- **Deliver:** notification module and tests.
- **Accept when:**
  - Each notification type can be turned off in settings.
  - Every error message says what happened and what the user can do.

### M7-14 Arabic review and four-variant audit (gate G5)

- **Status:** todo
- **Depends on:** M7-01, M7-02, M7-03, M7-04, M7-05, M7-06, M7-07, M7-08, M7-09, M7-10, M7-11, M7-12, M7-13, Q-06
- **Docs:** [08 Four variants, always](../08-gui.md#four-variants-always), D-015
- **Do:** Audit every screen in all four variants against a checklist: mirroring, icon direction, technical-text isolation, truncation and overflow with long Arabic strings, digit style, contrast, focus order. Have a fluent Arabic reviewer check all strings for correctness and consistent terminology against the glossary. Fix everything found.
- **Deliver:** `docs/reference/four-variant-audit.md` with the checklist and results, corrected strings and layouts.
- **Accept when:**
  - Every screen passes every checklist item in all four variants.
  - The reviewer signs off the Arabic strings and glossary.
