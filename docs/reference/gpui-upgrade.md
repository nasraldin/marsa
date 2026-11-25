# Upgrade GPUI and gpui-component together

Marsa's native macOS GUI uses GPUI 0.3.13 and gpui-component 0.10.0. Both
application pins are in the workspace manifest. The
[vendored source tree](https://github.com/nasraldin/marsa/tree/main/third-party/gpui)
contains the checksum-verified crate releases followed by the compatibility
commits, recorded in `UPSTREAM.tsv` with each registry archive's SHA-256
digest. The sources live directly in the main repository under
`third-party/gpui`; a fresh clone needs no extra initialization step.

1. Read the released GPUI and Component source for the matching revisions.
   Verify each crate archive against the crates.io checksum and preserve its
   upstream licence and notices.
2. Import those releases into `third-party/gpui` and replay the
   documented fixes. Remove a patch when the released upstream source contains
   an equivalent fix. Update the workspace pins and lockfile together.
3. Run both locked audits with the existing workspace policy.

   ```sh
   cargo deny --locked check
   cargo deny --locked --target aarch64-apple-darwin check
   audit_metadata=$(mktemp)
   python3 .github/scripts/audit-path-dependencies.py metadata > "$audit_metadata"
   cargo deny --locked --metadata-path "$audit_metadata" --config deny.toml check -D warnings advisories
   rm -- "$audit_metadata"
   ```

4. Run workspace tests and clippy. Open the native shell and verify closure
   exits the process. Recheck English light, English dark, Arabic light, and
   Arabic dark. Re-run keyboard, Arabic caret and selection, live IME, and
   VoiceOver qualification before releasing a new pin.

RustSec skips path packages in its default scans. The supplemental check uses
the exact resolved graph and source manifest to audit the original published
versions. Scheduled Cargo Audit also checks those versions against the
unchanged RustSec requirements.

The native macOS graph retains the CoreText font backend, the Metal renderer,
and AccessKit. The Component branch keeps Linux, Windows, and web dependencies
behind explicit features for future work on those platforms. The unfiltered
audit still checks the full resolved Marsa workspace.

The CoreText backend applies the OpenType `wght` axis after FontKit selects a
face from a bundled family. FontKit's in-memory source loads the Raleway
variable font at its Thin default. The Marsa native typography test checks
that the selected font resolves to the Regular metrics and that Medium and
Semibold resolve to distinct faces. It also covers bundled static fonts and
the System terminal font.

The date components use Jiff civil dates and date-times and retain the default
date formats and calendar behavior. Jiff's civil date range is ±9,999 years
and some custom format directives differ from Chrono. The LSP URI adapter
preserves the original spelling of valid references. The HTML renderer uses
the maintained html5ever parser with a private tree adapter.

DataTable scopes its accessibility root to the table state entity. Keep this
identity stable across redraws and focus changes while allowing multiple tables
in one window. The native `data_table_render` test verifies both properties.
