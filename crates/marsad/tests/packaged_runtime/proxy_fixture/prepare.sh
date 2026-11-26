#!/bin/sh
# Build only a disposable VZ guest and a networkless EROFS formatter.
set -eu
umask 077
if [ "$#" -lt 3 ] || [ "$#" -gt 4 ] || { [ "$#" -eq 4 ] && [ "$4" != "--intercept" ]; }; then
    printf 'Usage: sh prepare.sh BASE_GUEST VERIFIED_ALPINE_TAR EMPTY_SCRATCH_DIR [--intercept]\n' >&2
    exit 2
fi
source=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$source/../../../../.." && pwd -P)
guest=$(cd "$1" && pwd -P)
alpine_dir=$(cd "$(dirname "$2")" && pwd -P)
alpine="$alpine_dir/$(basename "$2")"
cd "$3"
if [ -n "$(ls -A)" ]; then
    printf 'The fixture requires an empty scratch directory.\n' >&2
    exit 2
fi
[ "$(go env GOVERSION)" = go1.27.4 ]

alpine_sha=$(sed -n '/^\[alpine\]/,/^\[/s/^sha256 = "\([^"]*\)"$/\1/p' "$repo/guest/versions.toml")
alpine_version=$(sed -n '/^\[alpine\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' "$repo/guest/versions.toml")
[ -n "$alpine_sha" ] && [ -n "$alpine_version" ]
printf '%s  %s\n' "$alpine_sha" "$alpine" | shasum -a 256 -c -
branch=$(printf 'v%s' "${alpine_version%.*}")
mkdir -p formatter/apk formatter/root
for repository in main community; do
    while read -r digest package; do
        case "$package" in
            ""|*[!a-zA-Z0-9_.+-]*|.*) exit 1 ;;
        esac
        apk="formatter/apk/$package"
        curl --proto '=https' --fail --silent --show-error \
            "https://dl-cdn.alpinelinux.org/alpine/$branch/$repository/aarch64/$package" -o "$apk"
        printf '%s  %s\n' "$digest" "$apk" | shasum -a 256 -c -
    done < "$repo/guest/image/builder/$repository/aarch64.sha256"
done
tar -xzf "$alpine" -C formatter/root
for archive in formatter/apk/*.apk; do
    tar -xzf "$archive" --exclude '.SIGN.*' --exclude '.PKGINFO' -C formatter/root
done
install -m 0755 "$source/formatter-init" formatter/root/init
bsdtar --uid 0 --gid 0 --format=newc -czf formatter/initramfs.gz -C formatter/root .

GOTOOLCHAIN=local GOPROXY=off GOOS=linux GOARCH=arm64 CGO_ENABLED=0 \
    go build -trimpath -o marsa-proxy-fixture "$source/endpoints.go"
shift 3
sh "$source/stage.sh" "$guest" "$PWD/guest-fixture" "$PWD/marsa-proxy-fixture" "$@"
# The leaf key stays private in the scratch guest. The CA private key is unnecessary.
rm "$PWD/guest-fixture/ca.key"
python3 "$source/format.py" "$guest"
