# M6 GUI foundation

**Outcome:** the design system (`marsa-ui`) with tokens, themes, direction and translations; the test harness for four variants; and the app shell.

Design: [08 GUI](../08-gui.md). [D-023](../15-decisions.md#d-023-choose-gpui-for-the-desktop) closes G1 with GPUI.

Until M4 is done, GUI work runs against the fake backend and, for real container data, any Docker-compatible engine reachable through a socket (a temporary bootstrap; see D-021).

---

### M6-01 GUI application skeleton

- **Status:** done
- **Depends on:** M1-05, M0-01, M6-14
- **Docs:** [08 Architecture](../08-gui.md#architecture), [08 Framework version policy](../08-gui.md#framework-version-policy)
- **Do:** Create `marsa-gui` and `marsa-ui`. Pin GPUI and gpui-component to the revisions validated in M1-01 and document the upgrade procedure. Open a window, handle app lifecycle, and exit the process when the window closes.
- **Deliver:** both crates, `docs/reference/gpui-upgrade.md`.
- **Accept when:**
  - The app opens an empty window and quits fully on close (no GUI process remains).
  - GPUI and gpui-component are pinned by exact revision in one place.

The Apple silicon dependency audit of GPUI Kit and gpui-component 0.10.0 with GPUI 0.3.12 reported five RustSec unmaintained dependencies (`instant`, `paste`, `ttf-parser`, `rustybuzz` and `chrono`), two MPL-2.0 dependencies outside the current allowlist (`cbindgen` and `option-ext`), and 21 duplicate-version groups. Kit and its components require several of these crates directly. The audit added the exact tested Kit version in an isolated worktree and ran `cargo deny --target aarch64-apple-darwin --locked check`. Resume with a maintained, policy-compliant GPUI dependency graph and re-run the input and lifecycle qualification for its pins. See [D-023](../15-decisions.md#d-023-choose-gpui-for-the-desktop).

Requalification on 2026-11-22 resolved the published [GPUI Kit 0.10.0](https://docs.rs/crate/gpui-kit/0.10.0) with only its `component` feature, and a separate graph using [gpui-pre 0.3.13](https://docs.rs/crate/gpui-pre/0.3.13) and [gpui-pre-platform 0.3.13](https://docs.rs/crate/gpui-pre-platform/0.3.13) with the macOS `font-kit` feature. Each resolved in a separate workspace copy and was checked with the existing `deny.toml` using `cargo deny --locked check` and `cargo deny --target aarch64-apple-darwin --locked check`.

| Resolved graph | Apple silicon duplicate groups | Apple silicon unmaintained crates | Apple silicon disallowed licences |
|---|---:|---:|---:|
| GPUI Kit with components | 21 | 5 | 2 |
| GPUI with its font-enabled platform crate | 17 | 3 | 2 |

Both Apple silicon graphs include the unmaintained `chrono`, `rustybuzz` and `ttf-parser` and the disallowed MPL-2.0 dependencies `cbindgen` and `option-ext`. The target-filtered run also reports two unmatched skips for existing non-Apple dependencies. Both unfiltered workspace runs independently fail the dependency policy. Neither candidate is eligible to enter the production workspace.

M6-14 reproduced a passing native GPUI core graph with source-level patches and the unmodified policy. AccessKit's binding migration is still open upstream and the current component release pins a different GPUI revision. That experiment covered the native core before the compatible Component port. See the [remediation evidence](../spikes/M6-14-gpui-dependency-remediation.md).


The native graph now pairs GPUI 0.3.13 with the ported gpui-component 0.10.0.
The source pin includes the macOS AccessKit update, Jiff civil dates,
the current html5ever and URI parsers, and the dependency version alignment
recorded in the [GPUI upgrade procedure](../reference/gpui-upgrade.md).
The standard graph audits use the existing policy. A supplemental advisory
audit preserves the original registry identity of the reviewed path patches.
On macOS 27.2.1 the native shell opened in English light, English dark,
Arabic light, and Arabic dark. The final window closed through the native
window control and Command-W, and the app process exited. Light appearance
was forced only in a disposable test bundle. M6-08 owns production keyboard,
Arabic editing and VoiceOver integration.

### M6-02 Design tokens, themes and fonts

- **Status:** done
- **Depends on:** M6-01, M6-13
- **Docs:** [08 Design tokens](../08-gui.md#design-tokens), [08 Themes](../08-gui.md#themes)
- **Do:** Define semantic tokens for colour, typography, spacing, radius, elevation and motion, each with light and dark values. Implement live theme switching and following the system appearance. Register and use the verified bundled font assets from M6-13. Add a contrast test for text and status colours in both themes.
- **Deliver:** token definitions and theme provider and font registration in `marsa-ui`.
- **Accept when:**
  - Switching the system appearance changes the app live when the setting is `system`.
  - The contrast test passes WCAG AA for every text and status token pair.
  - No colour literal exists outside the token definitions (lint or test).

On macOS 27.2.1 the native workspace opened in English light and dark
and Arabic light and dark. A disposable process changed AppKit window
appearance from dark to light and back while the window stayed open.
Marsa remained in System mode and both the workspace and GPUI component
palettes followed each change. The bundled IBM Plex fonts were active in
both languages. Token tests cover text, badges, status roles, and chart
contrast. A Rust token check rejects authored colour literals in desktop
source while permitting sRGB values received from terminal output.

### M6-13 Qualify and stage the Figma typography assets

- **Status:** done
- **Depends on:** M0-01
- **Docs:** [08 Design tokens](../08-gui.md#design-tokens), V-16
- **Do:** Identify the fonts and weights from the Figma Foundations styles. Verify their redistribution licences and exact released native font files. Stage the files and provenance independently of GPUI registration.
- **Deliver:** pinned original native fonts, upstream licence notice, provenance and checksums.
- **Accept when:**
  - Each checked-in file matches the pinned upstream release and carries its redistributable licence.
  - Latin and Arabic styles cover regular, medium and semibold. Monospace covers technical Latin text. Arabic joins in an offline shaping check.
  - Font registration, application rendering and four-variant visual checks stay in the GUI tasks.

### M6-03 Direction layer

- **Status:** done
- **Depends on:** M6-01
- **Docs:** [08 Direction layer](../08-gui.md#direction-layer), D-015
- **Do:** Implement the start and end layout API wrapping the framework's styling, automatic row reversal, the directional-icon mirroring registry, and a left-to-right isolation helper for technical text. Add a lint or test that fails the build if screen code uses left or right layout properties directly.
- **Deliver:** direction module in `marsa-ui`, the lint, unit tests.
- **Accept when:**
  - The same layout code renders mirrored when the direction changes, with no per-screen branching.
  - The lint catches a deliberate left or right usage in a test fixture.
  - Technical text inside an Arabic row renders left-to-right.

The native direction test draws the same GPUI row on macOS in all four
language and appearance combinations. Its first and second cell centers
are 143 and 303 points in English, and 257 and 97 points in Arabic. CoreText
glyph checks verify that the path and command option read left to right
inside the Arabic sentence. The GUI build guard rejects a fixture with
physical padding, inset, alignment, corner and popover styles. The
production workspace was checked in all four variants on macOS 27.2.1.
The expanded sidebar toggle mirrors to the content edge and clears
the window controls. Command-K and Escape open and dismiss the palette
in both languages.

### M6-04 GUI translations and locale formatting

- **Status:** done
- **Depends on:** M6-01, M6-12
- **Docs:** [08 Internationalisation](../08-gui.md#internationalisation), V-10
- **Do:** Integrate the Fluent catalogs with the component layer. Verify how gpui-component localises its own strings and how they coexist with Fluent (V-10). Implement locale-aware date, time and size formatting with Western digits and live language switching. Create `locales/GLOSSARY.md` for technical terms and arrange Arabic terminology review.
- **Deliver:** GUI i18n integration in `marsa-ui`, an updated V-10 entry, locale formatting and switching tests.
- **Accept when:**
  - Switching language at runtime updates text and direction without a restart.
  - gpui-component's built-in strings appear in the selected language.
  - Dates, times and sizes follow the selected locale and use Western digits.

GPUI Component 0.10.0 calls rust-i18n and exports a runtime locale setter.
Its upstream catalog contains English but no Arabic. rust-i18n 4.2.4 exposes
a crate-scoped catalog extension, which can add translations without
replacing Marsa's Fluent catalog.
[Pinned GPUI Component setup](https://github.com/longbridge/gpui-kit/blob/f697c3133d55c64fa8a5b67926147fb3dadd276d/crates/component/src/lib.rs#L122-L160)
· [Pinned upstream catalog](https://github.com/longbridge/gpui-kit/blob/f697c3133d55c64fa8a5b67926147fb3dadd276d/crates/component/locales/ui.yml)
· [rust-i18n 4.2.4 release](https://crates.io/crates/rust-i18n/4.2.4).

**Qualification.** The build checks Arabic against 122 pinned English
messages and two literal component lookups. Removing a key and renaming an
interpolation placeholder each failed the check. Native GPUI tests switched
English to Arabic and back in one application, updating Fluent and component
labels and reversing logical row order. Unit tests cover Gregorian date
order, UTC conversion, both hour cycles and SI and IEC size boundaries.
The four snapshots use native GPUI dialogs, the Settings clock choices,
and the shared date, chart clock, size and transfer-rate formatters. Escape
dismissed the dialog in each variant.

[English light](../screenshots/m6-04/en-light.png) ·
[English dark](../screenshots/m6-04/en-dark.png) ·
[Arabic light](../screenshots/m6-04/ar-light.png) ·
[Arabic dark](../screenshots/m6-04/ar-dark.png).

The [technical glossary](../../locales/GLOSSARY.md) is ready for the
owner's Arabic terminology review under M7-14, as required by D-035.

### M6-05 Core components

- **Status:** done
- **Depends on:** M6-02, M6-03, M6-04
- **Docs:** [08 Design system](../08-gui.md#design-system-marsa-ui)
- **Do:** Build Marsa's component set on gpui-component, each using tokens, direction and translations: button, icon button, text input, search field, select, checkbox, switch, tabs, list and virtualised list, table and virtualised table, tree, dialog, confirmation dialog with typed confirmation, toast, menu, tooltip, badge and status pill, progress, empty state, error state, code and monospace block, split view. Add a component gallery app.
- **Deliver:** components in `marsa-ui`, and the gallery as an example binary under `crates/marsa-ui/examples`.
- **Accept when:**
  - Every component appears in the gallery in four variants.
  - Every component is fully operable by keyboard.

Run the gallery with `cargo run --locked -p marsa-ui --example gallery -- en-US light`.
The gallery also accepts `ar` and `dark`.

Native qualification covered keyboard activation, text and search, select,
checkbox and switch, tabs, list and table selection, tree navigation, dialogs,
toast dismissal, menus, help, copy, split resizing, and page scrolling. The
Images route was also checked with three isolated sample images in all four
variants. The native `component_render` test covers independent table
accessibility identities, measured split resizing, and keyboard reachability of
toast dismissal.

[English light](../spikes/M6-05-component-gallery/en-light.png),
[English dark](../spikes/M6-05-component-gallery/en-dark.png),
[Arabic light](../spikes/M6-05-component-gallery/ar-light.png),
[Arabic dark](../spikes/M6-05-component-gallery/ar-dark.png).

### M6-06 Four-variant snapshot harness

- **Status:** todo
- **Depends on:** M6-05
- **Docs:** [13 GUI tests](../13-testing-and-benchmarks.md#gui-tests)
- **Do:** Build the snapshot test harness that renders any component or screen in English light, English dark, Arabic light and Arabic dark, compares against committed references, and produces a visual diff. Add layout assertions for right-to-left.
- **Deliver:** the harness, snapshots for every M6-05 component, CI integration.
- **Accept when:**
  - A deliberate visual change fails CI with a diff image.
  - Each component has four committed reference images.

### M6-07 App store and services

- **Status:** done
- **Depends on:** M6-01, M5-01
- **Docs:** [08 Data flow](../08-gui.md#data-flow)
- **Do:** Implement the normalised app store with selectors, and the services layer: daemon client, Docker client, snapshot-then-events loading, batching and rate-capping of log and stats streams, command dispatch, and cancellation of all streams on window close.
- **Deliver:** store and services modules in `marsa-gui` with tests using fake clients.
- **Accept when:**
  - A burst of 100,000 log lines is delivered to the view layer in bounded batches without blocking the UI thread.
  - Views re-render only when their selected state changes (tested with counters).
  - Closing the window cancels every stream.

The owner socket delivers an ordered daemon snapshot and events, including
Docker inventory invalidations from a passive observer. The window refreshes
the relevant store from the owner API and retains unchanged projections for
selectors. A separate app-owned observer delivers approval and engine
notifications when all windows are closed, as required by M7-12 and M7-13.

- **Evidence:** Fake clients delivered 100,000 ordered lines under the
  batch limits, coalesced stats, counted route-specific redraw decisions, and
  dropped daemon, log, and stats streams when the window closed with live
  handles. The fake guest observed Docker events while still reaching idle
  shutdown. The signed Apple silicon packaged test on macOS 27.2.1 observed a
  Docker volume creation on the owner stream and read the new volume through
  the passive inventory API. The native desktop reached Containers with
  Command-1 in both languages. Command-W released the window's owner
  connection while the background notification connection stayed open.
  Reopening restored the window connection. Reviewed Volumes captures:
  [English light](../spikes/M6-07-app-store/en-light.png),
  [English dark](../spikes/M6-07-app-store/en-dark.png),
  [Arabic light](../spikes/M6-07-app-store/ar-light.png), and
  [Arabic dark](../spikes/M6-07-app-store/ar-dark.png).
- **Follow-up:** [M7-48](M7-gui-features.md#m7-48-drive-overview-inventory-from-docker-invalidations)
  moves the Overview inventory projection off the live utilization timer.

### M6-08 Keyboard and accessibility foundation

- **Status:** todo
- **Depends on:** M6-05
- **Docs:** [08 Accessibility](../08-gui.md#accessibility), V-11
- **Do:** Verify the state of GPUI's accessibility support on macOS (V-11). Implement focus management, visible focus, direction-aware arrow navigation, a central shortcut registry, and accessibility labels on every component as far as the framework supports. Honour reduce-motion.
- **Deliver:** focus and shortcut modules, the Rust-backed AppKit editor qualified in the product window, and an updated V-11 entry documenting screen reader behavior.
- **Accept when:**
  - Every component is reachable and operable by keyboard in both directions.
  - Physical arrow keys move the caret visually in Arabic, and Shift-arrow selects the expected text.
  - Live IME candidate positioning, selection and commit work in the native editor.
  - VoiceOver can navigate to the editor from a GPUI control and speak its label, value and editing feedback.
  - The accessibility finding and any gaps are documented, with follow-up tasks created.

### M6-09 App shell

- **Status:** in progress
- **Depends on:** M6-05, M6-07, M6-08
- **Docs:** [08 Screens](../08-gui.md#screens)
- **Do:** Build the shell: sidebar navigation, routing, content area, status bar with the engine indicator, global search entry point, the command palette listing every registered action, and the toast and error surface.
- **Deliver:** shell in `marsa-gui`, snapshots in four variants.
- **Accept when:**
  - Navigation works by mouse and keyboard; the sidebar is on the right in Arabic.
  - The command palette can run every registered action and is searchable in both languages.
  - The engine indicator reflects daemon events live.

Compact navigation keeps centered route icons and reserves room for macOS
window controls. The toggle mirrors with the reading direction. The workspace
glow and rail use the same active accent as the controls.


### M6-10 Tray item

- **Status:** done
- **Depends on:** M3-12, M6-01, M8-02, Q-06
- **Docs:** [03 Responsibilities](../03-host-daemon.md#responsibilities), D-003
- **Do:** Implement the menu-bar item owned by `marsad`: engine state, start and stop, open the GUI, pending agent approvals count, quit. Localised and following the system appearance.
- **Deliver:** tray module in the macOS platform code.
- **Accept when:**
  - The tray works with no GUI process running.
  - Its menu follows the selected language and system appearance in English light, English dark, Arabic light and Arabic dark.
  - The displayed engine state and pending approvals count update from daemon events.
  - Start and Stop control the packaged VM. Open launches the Marsa GUI and Quit shuts down the daemon and its VM cleanly.

M3-12 installs a VM supervisor behind the engine API. M6-01 supplies the
launchable GUI. M8-02 owns the approval queue and publishes its updates.
The tray stays in the daemon and runs with the GUI closed.

The daemon installs an AppKit status item in its graphical login session.
Native menu labels, direction and the template image follow the selected
language and AppKit appearance. The signed Apple silicon qualification used
the production menu selectors and daemon runtime with a disposable 1 GiB
userspace-network VM and pinned Docker 29.10.0. It proved the status item
without a desktop process, the approval count changing from 0 to 1 and
back, and all four language and appearance variants. Stop canceled an
in-progress boot, Start reached Ready, Open launched the packaged desktop,
and Quit stopped the running VM and removed the private sockets and PID
file. The ext4 superblock had a clean state with no journal replay required.
A separate native confirmation test sent SIGTERM while the dialog was open
and verified exit and socket cleanup. [M3-19](M3-vm-and-daemon.md#m3-19-qualify-default-capacity-cold-boot)
tracks the timeout during preparation of a fresh default-capacity disk.

### M6-11 Onboarding

- **Status:** in progress
- **Depends on:** M6-09, M4-04, M4-05
- **Docs:** [08 Screens](../08-gui.md#screens), [07 The allowlist](../07-file-sharing-and-storage.md#the-allowlist)
- **Do:** Build first-run onboarding: language, theme, choosing folders to share (nothing preselected beyond a suggestion the user confirms), Docker context setup, and optional CLI links on `PATH`.
- **Deliver:** onboarding flow, snapshots in four variants.
- **Accept when:**
  - Finishing onboarding leaves a working `docker` CLI with the `marsa` context.
  - No folder is shared without the user choosing it.
  - The flow can be completed by keyboard only.

The workspace follows the Figma language, appearance, sharing and Docker setup
frames. Host API revision 12 persists completion. Setup adds only the folders
the user chose as read-only shares, and explicitly registers the local Docker
context. CLI link management and four-variant acceptance remain open.

### M6-12 Shared Fluent catalogs and typed keys

- **Status:** done
- **Depends on:** M0-01, M5-02
- **Docs:** [08 Internationalisation](../08-gui.md#internationalisation), [13 Definition of done](../13-testing-and-benchmarks.md#definition-of-done-for-any-task)
- **Do:** Add framework-independent Fluent catalogs for English (US) and Arabic. Validate both catalogs at build time and generate typed keys. Use a key for an existing CLI message. Exercise all six Arabic plural categories in test catalogs using the same parser and runtime as production.
- **Deliver:** `marsa-l10n`, `locales/en-US/`, `locales/ar/`, and tests.
- **Accept when:**
  - A missing key, mismatched argument or invalid catalog fails build validation.
  - All six Arabic plural categories are exercised in a Fluent fixture.
  - The migrated CLI message preserves its English output and JSON behavior. Desktop messages retain both locales.

### M6-14 Qualify replacements for blocked GPUI dependencies

- **Status:** done
- **Depends on:** M1-05, M0-07
- **Docs:** [08 Framework version policy](../08-gui.md#framework-version-policy)
- **Do:** Replace advisory and license blockers in an isolated GPUI graph. Test Apple rendering against the patched graph and run the original strict workspace and Apple target checks. Keep the production workspace unchanged until the graph is compliant.
- **Deliver:** Exact upstream versions, replacement evidence, residual dependency chains, and the next integration requirements.
- **Accept when:**
  - The replacement graph builds using native Apple text and Metal and the evidence states which interactive tests remain.
  - The same unmodified policy reports the exact blockers for the complete workspace and Apple target before and after the experiment.
- **Evidence:** The [remediation report](../spikes/M6-14-gpui-dependency-remediation.md) records the replacements, native rendering and protocol results, and the strict dependency checks. The production GUI integration remains M6-01.
