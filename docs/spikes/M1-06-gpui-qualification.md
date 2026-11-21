# M1-06 GPUI desktop qualification

This report supports the framework decision in M1-05. It tests representative UI paths before the Docker engine, app store and release application exist. Each fixture uses synthetic container data. The framework gate remains with the owner.

## Environment

The probes used a physical Apple silicon Mac, macOS 27.2.1, Rust 1.100.0 and the existing standalone GPUI workspace. GPUI Kit and GPUI Component 0.10.0 use GPUI snapshot 0.3.12 in this lockfile. GPUI Kit 0.10.0 was the latest compatible stable suite when tested. The offscreen tests use CoreText and Metal. The load and lifecycle probes use the native macOS event loop. [GPUI Kit release](https://github.com/longbridge/gpui-kit/releases/tag/v0.10.0) · [Previous spike and Figma typography](M1-01-gpui-rtl.md).

## Five RTL criteria

| Criterion | Stock result | Evidence |
|---|---|---|
| Mirrored shell and list | Pass | The sidebar and container columns mirror in both themes, with the same control layout in all four variants. |
| Arabic text editing | Fail | Pure Arabic typed into Kit's Input. Three positions shared the same rendered caret x and a click on the visual starting edge chose the end offset. The expanded probes below test possible remedies. |
| Mixed labels and technical values | Pass | The Arabic groups follow the paragraph direction. Isolated container name, image reference and path run left to right. |
| Arabic shaping | Pass | The nine weight and size combinations in the typography probe produce connected glyphs. |
| Live language switching | Pass | The same window switches from English to Arabic and back and moves the sidebar and columns. |

## Arabic editor options

- **Stock GPUI Kit Input.** Arabic and mixed-script text were accepted. Logical selection, grapheme deletion and synthetic marked-text replacement passed. Supplying the correct logical position through the public selection setter still left several different positions painted at the same x coordinate. A mixed Arabic and Latin sample collapsed 24 of its 25 grapheme boundaries onto one x coordinate.
- **CoreText geometry in Rust.** The standalone geometry probe returned distinct visual positions for all four boundaries in `عرب`. It resolved the expected hit at the visual starting edge and exposed two valid caret positions at each tested mixed-direction boundary. A complete GPUI editor fix would need the visual mapping for painting, hit testing, movement and selection together.
- **Native AppKit editor embedded from Rust.** The probe used GPUI's public native window handle to attach an `NSTextView` to a hidden native GPUI window and give it first-responder status. AppKit Right actions produced distinct, correctly ordered caret positions in `عرب`. The text hit test, shifted selection and deletion, movement within the Latin run of a mixed sample, and programmatically driven marked-text composition also passed. Stock AppKit Backspace removed the final Arabic combining mark before the base letter. A small Rust subclass preserved the Arabic grapheme and the emoji ZWJ cluster while delegating deletion and composition to AppKit. [GPUI Kit native extension guide](https://gpui-kit.com/docs/native-extension/).

The embedded editor is a credible macOS path for keeping GPUI without modifying GPUI itself. The probe invoked native actions and text-service APIs in a hidden window. A visible, Figma-aligned field still needs qualification for physical Arabic keyboard and pointer input, actual IME candidate behavior, layout and clipping, popup layering, native and GPUI focus handoff, and VoiceOver. A GPUI-level fix would require upstream geometry and editor changes, with broader platform reach if accepted.

## Keyboard and accessibility

The Metal-backed Controls probe exercised an initial Start focus, forward and reverse tab traversal, disabled-button skipping, Space and Enter activation, dialog focus retention, cancellation and focus restoration. In each of the four variants an empty or incorrect removal token produced no action. The correct token produced exactly one action. Dropdown Escape, arrow-key selection and disabled-item skipping were also checked. These are fixture callbacks.

The Kit accessibility snapshots exposed localized names, roles, disabled state and the confirmation field's value. The pinned GPUI macOS source installs an AccessKit adapter for the native window. This host did not grant the test process external Accessibility access, so native accessibility traversal and spoken VoiceOver output remain to be exercised with a packaged app. [Kit accessibility guide](https://gpui-kit.com/docs/accessibility/).

## Component localization, charts and lifecycle

With Kit's locale set to `ar`, the stock Select still rendered and exposed “Please select”. An explicit placeholder rendered the Arabic translation and exposed it as the control value. This proves one component override. Catalogue completeness, all other built-in strings and Arabic plural rules remain work for M6-04.

Visual inspection of the four Metal captures confirmed that the chart's time axis reads left to right in both language directions, with Western digits. The chart labels were reviewed in the images. The automated assertion checks the Select values. [English light](M1-06-gpui-qualification/en-light.png) · [English dark](M1-06-gpui-qualification/en-dark.png) · [Arabic light](M1-06-gpui-qualification/ar-light.png) · [Arabic dark](M1-06-gpui-qualification/ar-dark.png).

The lifecycle probe opened a hidden native GPUI window, requested programmatic removal of its last window, observed the close callback and verified that its view was released before the process exited. It exercised no daemon streams.

## List and stream load

The native 20-second fixture consumed 200,000 synthetic lines from a producer scheduled to send 100 lines every 10 ms. The virtual list requested all 2,000 container rows over the scroll sequence and requested the final log row, line 200,000. Each list requested at most 29 rows at a time. The ring held at most 10,000 lines and 905,600 payload bytes. The queue allowed four batches, and ingestion drained at most eight batches per render callback.

After two seconds of warmup, 1,077 render-entry intervals measured 16.6 ms median, 16.7 ms p95 and 16.83 ms maximum. RSS sampled every two seconds stayed approximately 116 MiB. These measurements cover GPUI render callback cadence under synthetic load. GPU presentation latency, the idle target for the release app and the one-hour soak all require separate measurements. The 8-second smoke run had one interval above 20 ms and a maximum of 23.68 ms.

## Must-have coverage

| Marsa area | What the spike can establish | Remaining product check |
|---|---|---|
| English and Arabic, light and dark | Four rendered baseline variants, live direction switching and shaped labels. Stock Arabic editing fails. | Qualify a corrected editor and review all packaged screens and translations. |
| Container list and groups | Virtual rows and scrolling under synthetic load. | Grouping, filter search, events and real container actions in M7-01. |
| Container details and inspect | Text layout and representative controls. | Daemon data, long technical values and redaction in M7-02. |
| Live logs | Short native Metal run under a paced synthetic load with fixed bounds. | Engine input, mixed-text ANSI rendering, search, cancellation and an hour of frame and memory measurements in M6-07 and M7-03. |
| Terminal | GPUI Kit's upstream maintainer places terminal emulation outside Kit's scope. | Qualify a compatible emulator separately, then test exec, resize, copy/paste, scrollback and mixed script in M7-04. [Upstream terminal decision](https://github.com/longbridge/gpui-kit/issues/2978). |
| Live stats | Representative chart rendering and direction check. | Daemon sampling, accessibility, units and bounded live history in M7-05. |
| Images | Select and virtual list primitives. | Search, pull progress and cancellation, image actions and error recovery in M7-06. |
| Volumes and networks | List and confirmation primitives. | Typed confirmations and daemon results in M7-07. |
| Compose projects | Container row and keyboard primitives. | Grouping, project controls, state and aggregated logs in M7-08. |
| Engine controls and resources | Buttons, keyboard and disabled states. | Lifecycle commands, errors and daemon limits in M7-09. |
| Settings and updates | Component selection and language and theme switching. | Full settings, persistence, permissions and updater integration in M7-10 and M10. |
| Disk cleanup | Synthetic dialog guarded by an exact token. | Preview agreement, reclaim progress, cancellation and engine actions in M7-11. |
| MCP approvals and activity | Dialog, keyboard and synthetic action guard. | Daemon policy, audit entries and approval identity in M7-12. |
| Tray and notifications | Native window close and product scope reviewed. The tray belongs to the daemon. | Tray actions, notifications and global error state in M6-10 and M7-13. |
| Command palette and onboarding | Keyboard focus, popup, selection and close behavior sampled. | Full command catalogue, unmanaged paths and onboarding flows in M6-09 and M6-11. |
| Accessibility and review | Kit tree properties and keyboard checks on sample controls. | Packaged VoiceOver, reduced motion, focus recovery, Arabic review and Figma matches in M6-08 and M7-14. |

## Reproduce

On Apple silicon, install IBM Plex Sans Arabic Regular, Medium and SemiBold as described by the [original spike](M1-01-gpui-rtl.md). Run from the repository root.

```sh
spike=spikes/gpui-rtl/Cargo.toml
evidence_dir=$(mktemp -d)
cargo run --locked --manifest-path "$spike" -- --capture "$evidence_dir/rtl"
cargo run --locked --manifest-path "$spike" --bin arabic-editing
cargo run --locked --manifest-path "$spike" --bin controls
cargo run --locked --manifest-path "$spike" --bin locale-chart -- "$evidence_dir/chart"
cargo run --locked --manifest-path "$spike" --bin window-lifecycle
cargo run --locked --manifest-path "$spike" --bin stream-load -- 20
cargo test --locked --manifest-path "$spike"
```

The stream fixture uses the debug profile and opens a native window during its run. Sample RSS with `ps -o rss= -p PID` from a separate shell. Its duration is configurable and the fixed history and cadence histogram remain bounded. All additional Rust bindings and Unicode segmentation are confined to this standalone probe workspace.

## Decision for discussion

The measured rendering, controls and representative workloads support keeping GPUI as a candidate. Qualify one visible Marsa text field using the embedded native editor before accepting G1. If its remaining validation exposes an integration cost the project cannot carry, compare another Rust UI framework using the same five criteria and native input tests.
