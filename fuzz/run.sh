#!/usr/bin/env bash
# Run the scheduled budgets against disposable copies of the committed corpus.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
output_parent="${1:?pass an output directory for the corpus and crash inputs}"
mkdir -p -- "$output_parent"
output_dir="$(mktemp -d "$(cd "$output_parent" && pwd)/run.XXXXXXXX")"
printf 'Fuzz output: %s\n' "$output_dir"

cargo fetch --manifest-path fuzz/Cargo.toml --locked
export CARGO_NET_OFFLINE=true

run_target() {
  local target="$1"
  local max_length="$2"
  local corpus="$output_dir/corpus/$target"
  local artifacts="$output_dir/crashes/$target"
  mkdir -p -- "$corpus" "$artifacts"
  cp -- fuzz/corpus/"$target"/* "$corpus/"
  cargo fuzz run --sanitizer none "$target" "$corpus" -- \
    -max_total_time=30 -timeout=5 -rss_limit_mb=1024 \
    "-max_len=$max_length" "-artifact_prefix=$artifacts/"
}

# Four header bytes, a payload at the one MiB limit, and the first rejected length.
run_target guest_control 1048581
# A settings document at the 256 KiB limit and the first rejected length.
run_target settings 262145
# Enough for both localized notes at their limits even with escaped JSON characters.
run_target update_manifest 131072
# One MiB of inspected ContainerCreate JSON and the first rejected length.
run_target docker_binds 1048577
