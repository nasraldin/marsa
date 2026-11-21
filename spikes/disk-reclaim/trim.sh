#!/bin/busybox sh
busybox mount -t virtiofs -o ro marsa-spike /mnt/marsa
. /mnt/marsa/tools.sh

phase() {
    printf '\nDISK_PHASE:%s\n' "$1"
    while ! test -f "/mnt/marsa/ack-$1"; do
        busybox sleep 1
    done
}

printf 'DISCARD_GRANULARITY='
busybox cat /sys/block/vdb/queue/discard_granularity
printf 'DISCARD_MAX_BYTES='
busybox cat /sys/block/vdb/queue/discard_max_bytes
mkfs.ext4 -F -E nodiscard,lazy_itable_init=0,lazy_journal_init=0 /dev/vdb
busybox mount -t ext4 -o nodiscard /dev/vdb /mnt/disk
busybox sync
phase formatted

busybox dd if=/dev/urandom of=/mnt/disk/retained bs=1M count=32
busybox dd if=/dev/urandom of=/mnt/disk/released bs=4M count=1024
busybox sync
phase filled
retained_before=$(busybox sha256sum /mnt/disk/retained)

busybox rm /mnt/disk/released
busybox sync
phase deleted

trim_status=0
fstrim --verbose /mnt/disk || trim_status=$?
printf 'TRIM_STATUS:%s\n' "$trim_status"
busybox sync
phase trimmed

retained_after=$(busybox sha256sum /mnt/disk/retained)
test "$retained_before" = "$retained_after"
printf '\nRETAINED_SHA256:%s\n' "${retained_after%% *}"
busybox umount /mnt/disk
e2fsck -fn /dev/vdb
busybox umount /mnt/marsa
