#!/bin/sh
# Overlay the published guest tar in a private directory and reuse the Tier 2 EROFS formatter.
set -eu
umask 077
if [ "$#" -ne 3 ]; then
    echo 'Usage: sh prepare.sh BASE_GUEST FORMATTER_INITRAMFS NEW_SCRATCH_DIR' >&2
    exit 2
fi
source=$(cd "$(dirname "$0")" && pwd -P)
base=$(cd "$1" && pwd -P)
initramfs=$(cd "$(dirname "$2")" && pwd -P)/$(basename "$2")
mkdir -m 0700 "$3"
cd "$3"
mkdir formatter guest-fixture
cp "$initramfs" formatter/initramfs.gz
python3 "$source/stage.py" "$base/image-rootfs.tar" guest-fixture/image-rootfs.tar
python3 "$source/../proxy_fixture/format.py" "$base"
