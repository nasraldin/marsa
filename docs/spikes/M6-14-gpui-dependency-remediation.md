# M6-14 GPUI dependency remediation

Marsa keeps [GPUI](../15-decisions.md#d-023-choose-gpui-for-the-desktop).
This work qualified replacements in disposable workspaces. The production
workspace still needs a complete passing graph before M6-01 can start.

## Baselines and replacements

Zed's stable [v1.29.3](https://github.com/zed-industries/zed/releases/tag/v1.29.3)
at `d01b134cddf5e4f71964deb2d6f81cdb38fcf1e7` still declares `chrono`,
`ttf-parser`, resvg 0.46, and the Apple backend's `cbindgen`. GPUI's
[macOS platform source](https://github.com/zed-industries/zed/blob/d01b134cddf5e4f71964deb2d6f81cdb38fcf1e7/crates/gpui_macos/src/platform.rs)
selects its no-op text system if built without `font-kit`. The experiment
kept the font feature enabled throughout.

The reproducible comparison used the [gpui-pre 0.3.13 snapshot](https://docs.rs/crate/gpui-pre/0.3.13),
which identifies Zed revision `125ca8672e928b19a62c2e4b761fb29fe2a34ae3`,
and the crate builds from the [previous audit](../plan/M6-gui-foundation.md#m6-01-gui-application-skeleton).

| Blocker | Replacement verified in scratch |
|---|---|
| `chrono` | Switched scheduler UTC and accessibility snapshot timestamps to `time` 0.3.55. Kept UTC fallback for unavailable local offsets and RFC 3339 millisecond formatting. |
| `rustybuzz` and `ttf-parser` | Updated resvg and usvg to 0.50.0, which shape SVG text with HarfRust and Skrifa. GPUI uses a Skrifa charmap for its direct glyph-coverage test. Fontdb 0.24.0 still embeds a limited set of legacy table readers, disclosed by its upstream changelog. |
| `cbindgen` | Reproduced the Apple shader header from the exact upstream source and used that header in the scratch build. The stitched shader source matched byte-for-byte. Metal compiled both and exposed the same 16 entry points. Any production source must test header and ABI drift on upgrades. |
| `option-ext` | Verified that the sole `dirs-sys` comparison can use standard `Option` equality. A separate macOS-only font-kit build kept the CoreText loader and excluded dependencies used by other platform implementations. |

[resvg 0.50.0 source](https://github.com/linebender/resvg/tree/v0.50.0)
· [fontdb 0.24.0 changelog](https://github.com/RazrFalcon/fontdb/blob/v0.24.0/CHANGELOG.md)

## Verification and remaining work

The macOS scratch build uses the same `MacPlatform` constructor as the
multi-platform GPUI entry point. The headless CoreText probe laid out and
rasterized English and Arabic glyphs with nonzero image data. Earlier
scratch builds compiled the Metal shader and rendered Arabic SVG text,
mixed Arabic and Latin technical text, and Arabic with emoji. The machine's
Xcode lacked the optional command-line Metal toolchain, so the probes used
GPUI's runtime shader compilation path. The GUI lifecycle, matching
gpui-component integration, the
four language and theme variants, and live input and accessibility acceptance
remain M6 work.

The baseline and scratch runs used cargo-deny 0.22.0 and the same Marsa
`deny.toml`. The baseline and intermediate comparison used
`cargo deny --offline --all-features check` with and without the Apple
target. The final core used `cargo deny --offline --locked check` and
`cargo deny --offline --locked --target aarch64-apple-darwin check`.
The production workspace keeps its existing constraints and has no GPUI
overrides.

| Dependency graph | Apple silicon duplicate / advisory / license errors | Unfiltered duplicate / advisory / license errors |
|---|---:|---:|
| Published GPUI core with font-enabled platform package | 17 / 3 / 2 | 41 / 6 / 3 |
| Replacement core with the multi-platform package | 11 / 0 / 0 | 37 / 4 / 1 |
| Patched core with direct macOS backend and native accessibility | 0 / 0 / 0 | 0 / 0 / 0 |

The last experiment keeps Apple's font stack and moves the desktop bootstrap
to the native macOS package. It consolidates supported image and Protobuf
dependencies without removing format handlers. The dependency changes include
itertools 0.15.0, tiny-skia-path 0.12.0, Rand 0.10.4, getrandom 0.4.3 and
miniz_oxide 0.9.1. The scratch graph uses Tonic 0.14.9 with Base64 0.23.1, Flume 0.12.0 with
Spin 0.12.3, and Hashbrown 0.15.5 with Foldhash 0.2.0. PNG 0.18.1 and EXR
1.74.3 both use miniz_oxide 0.9.1.
The final Tempfile refresh selected the already-used getrandom 0.4.3 with its
published dependency range. The existing dependency skips were untouched.

The remaining macOS binding conflicts required an AccessKit source port.
The experiment replayed the macOS changes from [AccessKit PR 616](https://github.com/AccessKit/accesskit/pull/616)
at `969c6ad889b39b947a3404d856b6a5f393bf6ded` onto the released
[accesskit_macos 0.27.2](https://docs.rs/crate/accesskit_macos/0.27.2),
preserving the released live-region, subrole and subclass behavior when
resolving conflicts. It uses objc2 0.6.5, objc2-app-kit and
objc2-foundation 0.3.2. The adapter and accesskit_consumer 0.39.2 both
use hashbrown 0.17.1. The final core passed the headless native text probe. A fresh replay on
Marsa commit `f79f2f3d3aed8f30236e6ae9c22391f486d17149` updated the guest
agent to the same Base64 0.23.1 and passed both audits and 30 protocol and
agent tests. The Apple target reports two existing unmatched-skip warnings
for non-Apple dependencies.

The AccessKit change is still open upstream. The verified graph uses local
source patches and does not qualify a published stack. GPUI Kit's latest
[gpui-component 0.10.0](https://docs.rs/crate/gpui-component/0.10.0) pins
GPUI 0.3.12 and still uses the unmaintained date, clock and token-pasting crates.
Before adding release dependencies, qualify a maintained component stack
against the same core, rerun both dependency checks, and re-run native window,
input and accessibility tests for the exact pins.

Component, Base and Assets must move together because the assets implement
GPUI's `AssetSource` trait. A build with only the version pins updated
reported 61 errors in gpui-base. The port includes changes
to path and layer painting APIs, cached path translation, and notification
dismissal. The component's public date controls also expose `chrono`
types. A compatible migration must cover those APIs and replace
`instant` and `paste` while preserving UI behavior. Run the full
dependency check on the standalone component stack after those ports.
