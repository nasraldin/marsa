# IBM Plex desktop font assets

These unmodified TrueType files match the English, Arabic and monospace roles in the [Marsa Figma Foundations](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=2-353). The desktop uses Sans for Latin UI text, Sans Arabic for Arabic UI text, and Mono for Latin technical values. Mixed-script log rendering uses Sans Arabic wherever Mono has no Arabic glyph.

| Family | Stable package | Upstream revision | Styles |
|---|---|---|---|
| IBM Plex Sans | `@ibm/plex-sans` 2.0.0 | [`2e90fe3`](https://github.com/IBM/plex/tree/2e90fe366fdc27c12682db13e5aa20ded3d90908/packages/plex-sans/fonts/complete/ttf) | Regular, Medium, SemiBold |
| IBM Plex Sans Arabic | `@ibm/plex-sans-arabic` 1.1.0 | [`1da12f0`](https://github.com/IBM/plex/tree/1da12f02587b630c07e92692d21492d722f53614/packages/plex-sans-arabic/fonts/complete/ttf) | Regular, Medium, SemiBold |
| IBM Plex Mono | `@ibm/plex-mono` 2.5.0 | [`2f9ba1b`](https://github.com/IBM/plex/tree/2f9ba1b25957d958db71a849e85d72e3ecfb845a/packages/plex-mono/fonts/complete/ttf) | Regular |

All three packages carry the same original [SIL Open Font License 1.1 notice](OFL-IBM-Plex.txt). The copyright notice and reserved name are preserved. Copy only the verified native font files and the accompanying notice into the app bundle. No package lifecycle scripts are required.

Run `shasum -a 256 -c SHA256SUMS` from this directory before packaging. For an upgrade, check the IBM package's stable version and corresponding upstream tag. Compare downloaded files to their Git blob IDs before updating the pinned commits, files and checksums. CoreText read all seven native font faces on Apple silicon macOS 27.2.1. HarfBuzz applied Arabic joining in all three Arabic weights without missing glyphs. In-app fallback and the four visual variants are checked when the fonts are registered in GPUI.
