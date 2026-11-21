#!/bin/sh
# Validate immutable guest inputs and publish only verified archives.
set -eu
umask 022
cd /output

scratch=$(mktemp -d .inputs.XXXXXXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
trap 'exit 1' HUP INT TERM

verify() {
    if ! printf '%s  %s\n' "$1" "$2" | sha256sum -c -; then
        printf 'Guest input checksum mismatch: %s\n' "$2" >&2
        exit 1
    fi
}

fetch() {
    digest=$1
    url=$2
    file=${url##*/}
    if [ -L "$file" ]; then
        printf 'Refusing symlink %s\n' "$file" >&2
        exit 1
    fi
    if [ -f "$file" ]; then
        verify "$digest" "$file"
    else
        printf 'Downloading %s\n' "$url"
        if ! wget -q -T 45 -O "$scratch/$file" "$url"; then
            printf 'Guest input download failed: %s\n' "$url" >&2
            exit 1
        fi
        verify "$digest" "$scratch/$file"
        mv -f -- "$scratch/$file" "$file"
    fi
    printf '%s  %s\n' "$digest" "$file" >> "$scratch/SHA256SUMS"
}

stage=$1
shift
case "$stage" in
    kernel)
        fetch "$1" "$2"
        fetch "$7" "$8"
        ;;
    rootfs)
        fetch "$3" "$4"
        fetch "$5" "$6"
        ;;
    image|all)
        fetch "$1" "$2"
        fetch "$3" "$4"
        fetch "$5" "$6"
        fetch "$7" "$8"
        mv -f -- "$scratch/SHA256SUMS" SHA256SUMS
        ;;
    *)
        printf 'Unknown guest build stage\n' >&2
        exit 2
        ;;
esac
