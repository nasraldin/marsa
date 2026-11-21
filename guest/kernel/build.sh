#!/bin/sh
# Build the verified Linux source without network or elevated privileges.
set -eu
umask 022
version=$1
if [ "$(uname -m)" != aarch64 ]; then
    printf 'The guest kernel requires an arm64 Docker builder.\n' >&2
    exit 1
fi
scratch=$(mktemp -d /tmp/marsa-kernel.XXXXXXXXXX)
publish=$(mktemp -d /output/.kernel.XXXXXXXXXX)
trap 'rm -rf -- "$scratch" "$publish"' EXIT
trap 'exit 1' HUP INT TERM

printf '%s  /output/linux-%s.tar.xz\n%s  /output/check-config.sh\n' "$2" "$version" "$3" | sha256sum -c -
tar -xf "/output/linux-$version.tar.xz" -C "$scratch"
cd "$scratch/linux-$version"
fragment=/usr/local/lib/marsa/config
export KBUILD_BUILD_TIMESTAMP="1970-01-01 00:00:00 UTC"
export KBUILD_BUILD_USER=marsa KBUILD_BUILD_HOST=guest KBUILD_BUILD_VERSION=1
make ARCH=arm64 KCONFIG_ALLCONFIG="$fragment" allnoconfig

# Moby reports the required section separately from optional runtime features.
gzip -c .config > config.gz
NO_COLOR=1 sh /output/check-config.sh config.gz > "$publish/check-config.txt" || {
    cat "$publish/check-config.txt" >&2
    exit 1
}
sed -n '/^Generally Necessary:/,/^Optional Features:/p' "$publish/check-config.txt"

# Every enabled option in the fragment should survive Kconfig resolution.
invalid=0
while IFS= read -r setting; do
    case "$setting" in
        CONFIG_*=y|CONFIG_*=m|CONFIG_*=*[0-9])
            if ! grep -Fxq "$setting" .config; then
                printf 'Kernel configuration did not retain %s\n' "$setting" >&2
                invalid=1
            fi
            ;;
    esac
done < "$fragment"
[ "$invalid" -eq 0 ]
if grep -E '^CONFIG_(MODULES|SOUND|DRM|FB|WLAN|USB_SUPPORT|ATA|SCSI)=[ym]$' .config; then
    printf 'Kernel configuration enabled an excluded subsystem.\n' >&2
    exit 1
fi
make -s -j "$(nproc)" ARCH=arm64 Image vmlinux
cp arch/arm64/boot/Image vmlinux "$publish/"
cp .config "$publish/kernel.config"
for artifact in Image vmlinux kernel.config check-config.txt; do
    mv -f -- "$publish/$artifact" "/output/$artifact"
done
