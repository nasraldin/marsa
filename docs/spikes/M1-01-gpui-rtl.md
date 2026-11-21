# M1-01 GPUI right-to-left spike

**Recommendation:** Use the Tauri fallback in D-005. Four of five criteria passed. The single-line input placed the caret and clicks incorrectly in a three-letter Arabic word. G1 remains an owner decision in M1-05.

## Test setup

- Apple silicon, macOS 27.2.1 (26B131), Xcode 27.2, Rust 1.100.0.
- GPUI Kit and gpui-component 0.10.0, source commit `f697c3133d55c64fa8a5b67926147fb3dadd276d`. Kit pins its GPUI distribution to `gpui-pre` 0.3.12, based on Zed commit `d2a4fb6ded7c470be25d1541281da3882495b65b`. Kit 0.10.0 was the latest stable component release when measured on 2026-11-21. The independently published GPUI snapshot 0.3.13 is outside the Kit release's exact dependency pin. [Kit release](https://github.com/longbridge/gpui-kit/releases/tag/v0.10.0) · [Pinned Zed source](https://github.com/zed-industries/zed/tree/d2a4fb6ded7c470be25d1541281da3882495b65b/crates/gpui).
- The open-window and offscreen modes use one view. In offscreen mode GPUI draws with the native Metal renderer and CoreText. The harness dispatches focused text, keyboard and pointer events through GPUI and reads the control's rendered caret bounds. The input source and physical macOS Arabic keyboard were outside this capture.
- IBM Plex Sans Arabic was installed locally in Regular, Medium and SemiBold. Arabic samples use the Figma sizes and weights. Technical values use the system Menlo face in the prototype. [IBM Plex](https://github.com/IBM/plex) · [Marsa Figma typography](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=2-353).

## Results

| Criterion | Result | Evidence |
|---|---|---|
| 1. Sidebar and container row mirror | Pass | Sidebar center moved from x=78 to x=842 in the 920-point window. Name, image and path column centers changed from `[297.25, 538, 778.75]` to `[622.75, 382, 141.25]`. The order is checked in light and dark. |
| 2. Arabic typing, caret, selection and deletion | Fail | Typing `عرب` inserts the correct text. Three Right presses move the logical UTF-8 caret through `[6, 4, 2, 0]`. Rendered caret x positions are `[736.0, 714.08, 714.08, 714.08]`. The caret should move to increasing x positions. Clicking near the visible right edge selects offset 6 where the logical start is offset 0. Shift-Left selects `ع` logically. Backspace removes that selection, Forward Delete removes the next letter, and Backspace removes `صّ` as one grapheme. |
| 3. Arabic labels with technical values | Pass | Each isolated token's glyphs advance left to right. In the Arabic paragraph, the first glyph x positions of `web-api`, `ghcr.io/acme/api:1.4.2` and `/work/app` were `[380.61, 142.32, 0.00]`. The elements keep their order while the paragraph uses the Arabic reading direction. |
| 4. Correct Arabic shaping | Pass | Verified three distinct Arabic faces and nine size/weight combinations, from 11/500 to 64/600. The connected forms of `ع` and `ر` differ from their zero-width-non-joiner controls. The naturally disconnected `ب` stays the same. The captured type specimens show the joined forms visually. |
| 5. Live language switch | Pass | The language button changes both the strings and positions on the same window from English to Arabic and back. English captures use the left sidebar and Arabic captures use the right sidebar. |

All four images are native Metal captures of the synthetic spike window at 1840 × 1420 pixels. [English light](M1-01-gpui-rtl/en-light.png) · [English dark](M1-01-gpui-rtl/en-dark.png) · [Arabic light](M1-01-gpui-rtl/ar-light.png) · [Arabic dark](M1-01-gpui-rtl/ar-dark.png).

## Upstream status and patch size

GPUI Kit's global direction and grapheme-aware editing merged on 2026-11-10 and shipped in 0.10.0. The layout change explicitly leaves bidi caret and hit testing to GPUI. A later merged Textarea change aligns multi-line paragraphs from their first strong character. Its single-line input behavior is unchanged. Zed's RTL tracking and line rendering bugs are open. The proposed GPUI bidi-aware line mapping closed without merging. [Kit logical layout](https://github.com/longbridge/gpui-kit/pull/3886) · [Grapheme edits](https://github.com/longbridge/gpui-kit/pull/3887) · [Textarea alignment](https://github.com/longbridge/gpui-kit/pull/4016) · [Zed RTL tracking](https://github.com/zed-industries/zed/issues/31102) · [Line rendering](https://github.com/zed-industries/zed/issues/7465) · [Bidi mapping proposal](https://github.com/zed-industries/zed/pull/57239).

The macOS backend feeds CoreText's visual glyph order into GPUI. With IBM Plex Sans Arabic at 17 points, CoreText returned glyph indices `[2, 1, 0]` and native caret x positions `[27.829, 19.686, 14.263, 0]` at the logical boundaries. GPUI's `LineLayout::x_for_index` selects the first glyph at or after the requested logical index. That rule produces `[0, 0, 0, 27.829]` on those visual glyphs and is unchanged in GPUI snapshot 0.3.13. [Apple text backend](https://github.com/zed-industries/zed/blob/d2a4fb6ded7c470be25d1541281da3882495b65b/crates/gpui_apple/src/text_system.rs) · [GPUI coordinate mapping](https://github.com/zed-industries/zed/blob/d2a4fb6ded7c470be25d1541281da3882495b65b/crates/gpui/src/text_system/line_layout.rs).

| Patch area | Scope and upstream prospects |
|---|---|
| GPUI line geometry | Carry a mapping from logical grapheme offsets to visual caret and hit-test positions with directional affinity. macOS could supply positions through CoreText. Shared renderers and visual/logical round-trip tests are needed for a cross-platform patch. |
| GPUI Kit editing and selection | Consume the visual map for movement and click placement. Paint each visual selection segment when the logical range crosses directions. Existing arrow mirroring and grapheme boundaries can stay. Both changes depend on a working GPUI mapping API. |

An app-level edge-alignment override cannot repair the editor mapping. Maintaining a replacement input widget or both upstream changes would exceed the small patch allowed by the spike. The spike tested the published source unchanged.

## Reproduce

Install IBM Plex Sans Arabic Regular, Medium and SemiBold from the IBM Plex project. Run from the repository root on Apple silicon.

```sh
cargo run --locked --manifest-path spikes/gpui-rtl/Cargo.toml -- --capture /tmp/marsa-gpui-rtl
cargo run --locked --manifest-path spikes/gpui-rtl/Cargo.toml -- --interactive
cargo deny --locked --manifest-path spikes/gpui-rtl/Cargo.toml --config spikes/gpui-rtl/deny.toml --target aarch64-apple-darwin check licenses advisories sources
```

`--capture` reports visual caret and pointer behavior and writes four PNGs. The recorded caret failure is an experimental result. A run fails if it cannot produce the captures or exercise the known interfaces.

GPUI Kit supplies the compatible macOS platform, styled input and test renderer. Its lockfile is scoped to the standalone spike. The root policy rejects two required MPL-2.0 dependencies. `cbindgen` 0.28.0 generates the Apple Metal shader bindings at build time. `option-ext` 0.2.0 enters through the Apple font-kit directory lookup and the component's asset embedding build path. The spike's policy permits only these two crate versions under MPL-2.0. Its licence, advisory and source checks pass for Apple silicon. The production workspace uses its permissive-only dependency policy. The spike binary is for local verification and is excluded from Marsa distribution.
