#!/usr/bin/env bash
# Build twice from a clean output directory and compare all published guest payloads.
set -euo pipefail

output=target/guest
first=$(mktemp -d)
trap 'rm -rf -- "$first"' EXIT

cargo xtask guest all
(cd "$output" && sha256sum -c SHA256SUMS)
mv -- "$output" "$first/guest"
cargo xtask guest all
(cd "$output" && sha256sum -c SHA256SUMS)

artifacts=(
    Image vmlinux kernel.config check-config.txt rootfs.tar rootfs-size.txt rootfs.img rootfs-input.sha256
    guest-manifest.json guest-sbom.cdx.json guest-sources.json guest-notices.tar SHA256SUMS
)
for artifact in "${artifacts[@]}"; do
    cmp -- "$first/guest/$artifact" "$output/$artifact"
done
(cd "$output" && sha256sum -- "${artifacts[@]}" > guest-artifacts.sha256)
