#!/usr/bin/env bash
set -euo pipefail

# The ordinary Tier 2 suite runs against the current guest alone. This release
# check takes archived guest artifacts to exercise an actual upgrade.
: "${MARSA_VZ_GUEST_DIR:?set to the assembled current guest directory}"
: "${MARSA_PREVIOUS_VZ_GUEST_DIR:?set to the archived previous guest directory}"
: "${MARSA_VZ_NETWORK_HELPER:?set to the built user-space network helper}"

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd -P)
export MARSA_VZ_GUEST_DIR
MARSA_VZ_GUEST_DIR=$(cd -- "$MARSA_VZ_GUEST_DIR" && pwd -P)
export MARSA_PREVIOUS_VZ_GUEST_DIR
MARSA_PREVIOUS_VZ_GUEST_DIR=$(cd -- "$MARSA_PREVIOUS_VZ_GUEST_DIR" && pwd -P)
test "$MARSA_VZ_GUEST_DIR" != "$MARSA_PREVIOUS_VZ_GUEST_DIR"
test -f "$MARSA_VZ_GUEST_DIR/rootfs.tar"
test -f "$MARSA_PREVIOUS_VZ_GUEST_DIR/guest-manifest.json"
test -x "$MARSA_VZ_NETWORK_HELPER"

mkdir -p "$repo/target"
fixture=$(mktemp -d "$repo/target/disk-compat.XXXXXXXX")
tag="marsa-disk-compat:${fixture##*.}"
cleanup() {
    docker image rm "$tag" >/dev/null 2>&1 || true
    rm -rf -- "$fixture"
}
trap cleanup EXIT
mkdir "$fixture/source"
printf '2\n' > "$fixture/source/schema-version"
printf 'protected future layout sample\n' > "$fixture/source/user-data"
docker import --platform linux/arm64 "$MARSA_VZ_GUEST_DIR/rootfs.tar" "$tag" >/dev/null
docker run --rm --pull=never --network=none --read-only --cap-drop=ALL \
    --security-opt=no-new-privileges --user "$(id -u):$(id -g)" \
    --mount "type=bind,source=$fixture,target=/fixture" \
    "$tag" /sbin/mkfs.ext4 -q -F -d /fixture/source /fixture/future.img 1G
export MARSA_FUTURE_DISK_FIXTURE="$fixture/future.img"

cd -- "$repo"
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad --test packaged_runtime \
    disk_schema:: -- --ignored --nocapture --test-threads=1
