#!/bin/sh
# Reuse the verified formatter and endpoints binary from prepare.sh.
set -eu
umask 077
if [ "$#" -ne 3 ]; then
    printf 'Usage: sh prepare_intercept.sh BASE_GUEST PREPARED_PROXY_SCRATCH NEW_SCRATCH_DIR\n' >&2
    exit 2
fi
scripts=$(cd "$(dirname "$0")" && pwd -P)
guest=$(cd "$1" && pwd -P)
prepared=$(cd "$2" && pwd -P)
mkdir -m 700 "$3"
cd "$3"
mkdir formatter
cp "$prepared/formatter/initramfs.gz" formatter/initramfs.gz
sh "$scripts/stage.sh" "$guest" "$PWD/guest-fixture" "$prepared/marsa-proxy-fixture" --intercept
rm guest-fixture/ca.key
python3 "$scripts/format.py" "$guest"
