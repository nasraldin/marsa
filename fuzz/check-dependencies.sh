#!/usr/bin/env bash
# Check the isolated fuzz graph with the project policy and its libFuzzer exception.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
policy="$(mktemp)"
trap 'rm -f -- "$policy"' EXIT
cat deny.toml - > "$policy" <<'TOML'

# libFuzzer is linked only into the isolated parser fuzzing binaries.
[[licenses.exceptions]]
name = "libfuzzer-sys"
version = "=0.4.13"
allow = ["NCSA"]
TOML

cargo deny --manifest-path fuzz/Cargo.toml --locked --config "$policy" check
