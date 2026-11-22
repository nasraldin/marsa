#!/bin/sh
# Build a guest test initramfs without reading a host home or starting a service.
set -eu
umask 077
if [ "$#" -ne 2 ]; then
    printf 'Usage: sh prepare.sh VERIFIED_ALPINE_TAR EMPTY_SCRATCH_DIR\n' >&2
    exit 2
fi
source=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$source/../../../../.." && pwd -P)
alpine_dir=$(cd "$(dirname "$1")" && pwd -P)
alpine="$alpine_dir/$(basename "$1")"
cd "$2"
if [ -n "$(ls -A)" ]; then
    printf 'The fixture requires an empty scratch directory.\n' >&2
    exit 2
fi
[ "$(go env GOVERSION)" = go1.27.4 ]
alpine_sha=$(sed -n '/^\[alpine\]/,/^\[/s/^sha256 = "\([^"]*\)"$/\1/p' "$repo/guest/versions.toml")
[ -n "$alpine_sha" ]
printf '%s  %s\n' "$alpine_sha" "$alpine" | shasum -a 256 -c -

mkdir root
tar -xzf "$alpine" -C root
GOTOOLCHAIN=local GOPROXY=off GOOS=linux GOARCH=arm64 CGO_ENABLED=0 \
    go build -trimpath -o root/relay "$source/relay.go"
install -m 0755 "$source/init" root/init
bsdtar --uid 0 --gid 0 --format=newc -czf initramfs.gz -C root .
