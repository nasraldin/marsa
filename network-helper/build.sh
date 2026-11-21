#!/bin/bash
# Build the helper from the pinned upstream source and its verified vendor tree.
set -euo pipefail

commit=6dbb5d4fa0033356c72a201346f8350594229868
checksum=03cf1d57ae8c9fccd9146b55616ed1da11f5cdd9de9d39740d3ce4b14f6abb3a
go_version=go1.27.4
helper=$(cd "$(dirname "$0")" && pwd -P)
if [[ $# != 1 ]]; then
    echo "usage: bash network-helper/build.sh OUTPUT" >&2
    exit 2
fi
mkdir -p "$(dirname "$1")"
output="$(cd "$(dirname "$1")" && pwd -P)/$(basename "$1")"

export GOTOOLCHAIN=local GOENV=off GOWORK=off GOFLAGS= GOPROXY=off GOSUMDB=off
if [[ $(go env GOVERSION) != "$go_version" || $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo "expected ${go_version} on Apple silicon macOS" >&2
    exit 1
fi

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
archive=${MARSA_NETWORK_HELPER_ARCHIVE:-"$scratch/source.tar.gz"}
if [[ -z ${MARSA_NETWORK_HELPER_ARCHIVE:-} ]]; then
    curl --proto '=https' --proto-redir '=https' --location --fail --silent --show-error --retry 2 \
        "https://codeload.github.com/containers/gvisor-tap-vsock/tar.gz/$commit" -o "$archive"
fi
printf '%s  %s\n' "$checksum" "$archive" | shasum -a 256 -c -
mkdir "$scratch/source"
tar -xzf "$archive" --strip-components=1 -C "$scratch/source"
cmd="$scratch/source/cmd/marsa-net-helper"
mkdir "$cmd"
cp "$helper/"*.go "$helper/"*.c "$helper/"*.h "$cmd/"
cd "$scratch/source"

export CGO_ENABLED=1 GOOS=darwin GOARCH=arm64 GOARM64=v8.0
export CC="$(xcrun --sdk macosx --find clang)"
export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
export MACOSX_DEPLOYMENT_TARGET=27.0
export CGO_CFLAGS='-O2 -g' CGO_LDFLAGS='' CGO_CPPFLAGS=''
if [[ ${MARSA_NETWORK_HELPER_TEST:-0} == 1 ]]; then
    go test -trimpath -buildvcs=false -mod=vendor ./cmd/marsa-net-helper
fi
go build -trimpath -buildvcs=false -mod=vendor -ldflags='-buildid= -linkmode=internal' \
    -o "$scratch/marsa-net-helper" ./cmd/marsa-net-helper
mv -f "$scratch/marsa-net-helper" "$output"
