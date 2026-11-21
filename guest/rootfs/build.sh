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

# Only the lockfile determines which packages are downloaded. No index or
# repository participates in the install, and apk still verifies signatures.
(
    cd "$work/apks"
    while read -r digest package; do
        case "$package" in
            ""|*[!a-zA-Z0-9_.+-]*|.*)
                printf 'Invalid guest package filename: %s\n' "$package" >&2
                exit 1
                ;;
        esac
        if ! wget -q -T 45 -O "$package" "$repository/$package"; then
            printf 'Guest package download failed: %s\n' "$package" >&2
            exit 1
        fi
        if ! printf '%s  %s\n' "$digest" "$package" | sha256sum -c -; then
            printf 'Guest package checksum mismatch: %s\n' "$package" >&2
            exit 1
        fi
    done < "$lock"
)
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

tar -cf "$output_scratch/rootfs.tar" -C "$root" .
size=$(wc -c < "$output_scratch/rootfs.tar")
printf 'rootfs.tar: %s bytes\n' "$size" > "$output_scratch/rootfs-size.txt"
mv -fT -- "$output_scratch/rootfs.tar" rootfs.tar
mv -fT -- "$output_scratch/rootfs-size.txt" rootfs-size.txt
cat rootfs-size.txt
