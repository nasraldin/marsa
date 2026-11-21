#!/bin/sh
# Assemble a guest tree from the verified minirootfs, local APKs and Docker archive.
set -eu
umask 022

alpine_source=$1
docker_source=$2
output_scratch=$3
alpine_archive=${alpine_source##*/}
docker_archive=${docker_source##*/}
architecture=${alpine_archive%.tar.gz}
architecture=${architecture##*-}
repository=${alpine_source%/releases/*}/main/$architecture
recipe=/usr/local/lib/marsa/rootfs
lock=$recipe/$architecture.sha256

work=$(mktemp -d /tmp/marsa-rootfs.XXXXXXXXXX)
trap 'rm -rf -- "$work"' EXIT
trap 'exit 1' HUP INT TERM
root=$work/root
mkdir "$work/apks" "$root" "$work/upstream"

# The APK files are authenticated before an offline, signed install.
/bin/sh "$recipe/fetch-packages.sh" "$repository" "$lock" "$work/apks"
tar -xf "$alpine_archive" -C "$root"
apk --root "$root" --arch "$architecture" --cache=no --network=no \
    --repositories-file /dev/null --logfile=no --scripts=no \
    --force-non-repository add --initdb --usermode "$work"/apks/*.apk

# Retain the package database for inventory. PID 1 owns guest startup, so
# remove the BusyBox entrypoints for boot, terminal logins and scheduled jobs.
rm -rf -- "$root/var/cache/apk"
rm -f -- "$root/etc/inittab" "$root/sbin/init" "$root/sbin/getty" "$root/usr/sbin/crond"

mkdir -p "$root/usr/local/bin" "$root/etc/docker" \
    "$root/var/lib/marsa" "$root/var/lib/docker" "$root/var/lib/containerd" \
    "$root/Users" "$root/Volumes" "$root/private" "$root/mnt"
cp "$recipe/daemon.json" "$root/etc/docker/daemon.json"
tar -xf "$docker_archive" -C "$work/upstream"
for binary in containerd containerd-shim-runc-v2 docker-init docker-proxy dockerd runc; do
    upstream=$work/upstream/docker/$binary
    installed=$root/usr/local/bin/$binary
    cp "$upstream" "$installed"
    chmod 0755 "$installed"
    cmp "$upstream" "$installed"
done

# Validate using the pinned daemon itself when the builder matches the guest.
if [ "$(apk --print-arch)" = "$architecture" ]; then
    PATH="$root/usr/local/bin:$PATH" dockerd --validate --config-file "$root/etc/docker/daemon.json"
fi

# Normalize staging metadata in the archive without chown or capabilities.
tar --numeric-owner --owner=0 --group=0 --sort=name --mtime=@0 \
    -cf "$output_scratch/rootfs.tar" -C "$root" .
size=$(wc -c < "$output_scratch/rootfs.tar")
printf 'rootfs.tar: %s bytes\n' "$size" > "$output_scratch/rootfs-size.txt"
mv -fT -- "$output_scratch/rootfs.tar" rootfs.tar
mv -fT -- "$output_scratch/rootfs-size.txt" rootfs-size.txt
cat rootfs-size.txt
