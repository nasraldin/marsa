# The host sends this after the real agent reports the engine ready.
test "$(busybox cat /proc/1/comm)" = marsa-init
busybox grep -Eq '^[^ ]+ / erofs ro(,| )' /proc/mounts
busybox awk '$2 == "/dev" && $3 == "devtmpfs" && $4 ~ /(^|,)nosuid(,|$)/ { found = 1 } END { exit !found }' /proc/mounts
test "$(busybox cat /var/lib/marsa/schema-version)" = 1
docker=/mnt/marsa-test/docker
image=$(busybox cat /mnt/marsa-test/hello-world.id)
"$docker" --host unix:///var/run/docker.sock load --input /mnt/marsa-test/hello-world.tar
"$docker" --host unix:///var/run/docker.sock run --pull=never --network=none --rm "$image" > /run/marsa/hello-world.txt
busybox grep -Fx 'Hello from Docker!' /run/marsa/hello-world.txt
while IFS= read -r stage; do
    printf 'MARSA_PID1_STAGE %s\n' "$stage"
done < /run/marsa/boot-timings
printf 'MARSA_HELLO_WORLD_OK\n'
