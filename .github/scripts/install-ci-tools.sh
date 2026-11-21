#!/usr/bin/env bash
# Install pinned, checksum-verified tools for the Tier 1 runner.
set -euo pipefail

deny_version="0.22.0"
protoc_version="36.2"
case "$(uname -s):$(uname -m)" in
  Darwin:arm64)
    deny_target="aarch64-apple-darwin"
    deny_sha="aa23e2f32eaf0b92efb36de3fc925f3e1b841fdafcc555ddab43d4d146cec251"
    protoc_target="osx-aarch_64"
    protoc_sha="9cd98a532c5c5e0c4161314de0225de27e4c8a323917b6ea7b1b714d3ae23466"
    ;;
  Linux:aarch64)
    deny_target="aarch64-unknown-linux-musl"
    deny_sha="4818d44cdcac285776336504fca722493ab634d3377b94a462e98aad3f41484a"
    protoc_target="linux-aarch_64"
    protoc_sha="8b8f18bd2b30346efbc698dd5a73dd7c805f3ef8380f6dfc95c768f3f1852f6a"
    ;;
  Linux:x86_64)
    deny_target="x86_64-unknown-linux-musl"
    deny_sha="98c56e6a408bfa5c3c60744b6015aaa6aa08f743fdeb88386d06fca035bb1786"
    protoc_target="linux-x86_64"
    protoc_sha="121f6c7afe1d4d0e3ea6aab9432038599250134cbf4474cb1167d2c7decd4278"
    ;;
  *)
    echo "Tier 1 does not have tool pins for this platform." >&2
    exit 1
    ;;
esac

tools_dir="$(mktemp -d "${RUNNER_TEMP:?}/marsa-tools.XXXXXXXX")"
cd "$tools_dir"

download() {
  local url="$1" archive="$2" sha256="$3"
  curl --fail --silent --show-error --location --retry 3 \
    --proto '=https' --proto-redir '=https' --output "$archive" "$url"
  printf '%s  %s\n' "$sha256" "$archive" | shasum -a 256 -c -
}

deny_release="cargo-deny-$deny_version-$deny_target"
download "https://github.com/EmbarkStudios/cargo-deny/releases/download/$deny_version/$deny_release.tar.gz" \
  "$deny_release.tar.gz" "$deny_sha"
tar -xzf "$deny_release.tar.gz"
"$tools_dir/$deny_release/cargo-deny" --version
printf '%s\n' "$tools_dir/$deny_release" >> "${GITHUB_PATH:?}"

protoc_archive="protoc-$protoc_version-$protoc_target.zip"
download "https://github.com/protocolbuffers/protobuf/releases/download/v$protoc_version/$protoc_archive" \
  "$protoc_archive" "$protoc_sha"
unzip -q "$protoc_archive" -d protoc
"$tools_dir/protoc/bin/protoc" --version
printf '%s\n' "$tools_dir/protoc/bin" >> "$GITHUB_PATH"
