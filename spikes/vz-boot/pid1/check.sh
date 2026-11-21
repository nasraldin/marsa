test "$(busybox cat /proc/1/comm)" = marsa-init
test "$(busybox cat /var/lib/marsa/schema-version)" = 1
for stage in mount_system data_disk shared_folders network agent containerd dockerd; do
    busybox grep -q "^$stage=" /run/marsa/boot-timings
done
(busybox sleep 1 & printf '%s\n' "$!" > /run/marsa/orphan.pid)
orphan=$(busybox cat /run/marsa/orphan.pid)
for attempt in $(busybox seq 1 10); do
    [ -e "/proc/$orphan" ] || break
    busybox sleep 1
done
test ! -e "/proc/$orphan"
/marsa-test-docker info >/dev/null
printf 'MARSA_PID1_TEST_OK\n'
