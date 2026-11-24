# Marsa Fluent catalogs

Place each message in both `en-US/marsa.ftl` and `ar/marsa.ftl`. Keep the
message ID and its Fluent variable names the same across locales. The
build validates both catalogs and generates the Rust keys exposed through
`marsa_client::l10n::keys`.

Use a generated `TextKey` for a single literal in both locales. Use a
`Catalog` and `Arguments` for messages with variables or selects. This
shared catalog accepts messages, variable placeables and selects. It
validates the variables recursively inside every select branch.

Run `cargo test --locked -p marsa-l10n` to check catalog validation and
Arabic plural selection.
