#!/bin/busybox sh
set -eu
export PATH=/usr/bin:/bin:/usr/sbin:/sbin

busybox mount -t proc proc /proc
busybox mount -t sysfs sysfs /sys
busybox mount -t devtmpfs devtmpfs /dev
if [ ! -e /marsa-builtin-kernel ]; then
    busybox modprobe virtio_blk
    busybox modprobe squashfs
    busybox mkdir -p /.modloop
    busybox mount -t squashfs -o ro /dev/vda /.modloop
    # Alpine's modloop contains every matching module, including the vsock transport.
    kernel_release=$(busybox uname -r)
    busybox mount -o bind "/.modloop/modules/$kernel_release" "/lib/modules/$kernel_release"
    busybox modprobe vmw_vsock_virtio_transport
    busybox modprobe virtiofs
    busybox modprobe virtio_net
    busybox modprobe virtio_balloon
    busybox modprobe virtio-rng
fi

test -b /dev/vdb
test "$(busybox cat /sys/block/vda/ro)" = 1
test "$(busybox cat /sys/block/vdb/ro)" = 0
busybox mkdir -p /mnt/marsa
busybox mount -t virtiofs -o ro marsa-spike /mnt/marsa
test "$(busybox cat /mnt/marsa/fixture)" = marsa-vz-fixture
busybox umount /mnt/marsa
printf '\nMARSA_DEVICES_READY\n'
/marsa-vsock-fixture &

export PS1='MARSA_SHELL# '
exec busybox setsid busybox sh -c 'exec busybox sh -i </dev/hvc0 >/dev/hvc0 2>&1'
