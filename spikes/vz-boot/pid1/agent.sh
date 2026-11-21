#!/bin/sh
set -eu
printf 'MARSA_DEVICES_READY\n'
/marsa-vsock-fixture &
: > /run/marsa/agent.ready
for attempt in $(busybox seq 1 30); do
    /marsa-test-docker info >/dev/null 2>&1 && break
    busybox sleep 1
done
/marsa-test-docker info >/dev/null
export PS1='MARSA_SHELL# '
busybox setsid busybox sh -c 'exec busybox sh -i </dev/hvc0 >/dev/hvc0 2>&1' &
shell_pid=$!
trap 'busybox kill -HUP "$shell_pid"; wait "$shell_pid" || :; exit 0' TERM INT
wait "$shell_pid"
