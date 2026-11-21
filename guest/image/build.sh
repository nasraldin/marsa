#!/bin/sh
# Import the staged tar directly to preserve ownership without a privileged mount.
set -eu
umask 022

if ! apk info --exists "erofs-utils=$1" >/dev/null; then
    printf 'The EROFS formatter version does not match guest/versions.toml.\n' >&2
    exit 1
fi

# Add the static guest programs with explicit modes and the same ownership as the base.
cp /rootfs.tar /output/staged-rootfs.tar
programs=$(mktemp -d /tmp/marsa-programs.XXXXXXXXXX)
trap 'rm -rf -- "$programs"' EXIT
cp /programs/marsa-init /programs/marsa-agent "$programs/"
tar --append --file=/output/staged-rootfs.tar --owner=0 --group=0 --numeric-owner \
    --mode=0755 --mtime=@0 --transform='s|^|./usr/bin/|' -C "$programs" marsa-init marsa-agent

# Fixed time, UUID, worker count and path ordering keep the packing step stable.
mkfs.erofs --quiet -b4096 -zlz4hc,level=12 -T0 -U00000000-0000-0000-0000-000000000000 \
    --workers=1 --sort=path --tar=f /output/rootfs.img /output/staged-rootfs.tar
fsck.erofs --extract /output/rootfs.img
printf 'rootfs.img: %s bytes\n' "$(wc -c < /output/rootfs.img)"
