#!/bin/sh
# Import the staged tar directly to preserve ownership without a privileged mount.
set -eu
umask 022

if ! apk info --exists "erofs-utils=$1" >/dev/null; then
    printf 'The EROFS formatter version does not match guest/versions.toml.\n' >&2
    exit 1
fi

# Fixed time, UUID, worker count and path ordering keep the packing step stable.
mkfs.erofs --quiet -b4096 -zlz4hc,level=12 -T0 -U00000000-0000-0000-0000-000000000000 \
    --workers=1 --sort=path --tar=f /output/rootfs.img /rootfs.tar
fsck.erofs --extract /output/rootfs.img
printf 'rootfs.img: %s bytes\n' "$(wc -c < /output/rootfs.img)"
