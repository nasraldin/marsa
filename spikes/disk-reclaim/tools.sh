#!/bin/busybox sh
# This script is sourced inside the throwaway guest after mounting the fixture share.
busybox mkdir -p /marsa-tools/etc/apk /mnt/disk
busybox cp -r /etc/apk/keys /marsa-tools/etc/apk/
# The installed packages are meant to disappear at shutdown. Signature checks stay enabled.
/usr/sbin/apk --root /marsa-tools --initdb --no-network --no-cache --no-scripts \
    --no-progress --force-non-repository add /mnt/marsa/packages/*.apk
export PATH=/marsa-tools/sbin:/marsa-tools/usr/sbin:$PATH
export LD_LIBRARY_PATH=/marsa-tools/usr/lib:/marsa-tools/lib
busybox modprobe ext4
