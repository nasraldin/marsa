#!/bin/sh
# Download only the APKs listed in a checksum lockfile.
set -eu

repository=$1
lock=$2
cd "$3"

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
