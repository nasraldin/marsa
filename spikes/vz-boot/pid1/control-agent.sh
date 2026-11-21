#!/bin/sh
set -eu
/marsa-control-agent &
agent_pid=$!
shell_pid=''
trap 'busybox kill -TERM "$agent_pid" || :; if [ -n "$shell_pid" ]; then busybox kill -HUP "$shell_pid" || :; wait "$shell_pid" || :; fi; wait "$agent_pid" || :; exit 0' TERM INT
# The host retries until the actual agent has opened its listener.
printf '\nMARSA_DEVICES_READY\nMARSA_VSOCK_READY\n'
for attempt in $(busybox seq 1 30); do
    /marsa-test-docker info >/dev/null 2>&1 && break
    busybox kill -0 "$agent_pid"
    busybox sleep 1
done
/marsa-test-docker info >/dev/null
export PS1='MARSA_SHELL# '
busybox setsid busybox sh -c 'exec busybox sh -i </dev/hvc0 >/dev/hvc0 2>&1' &
shell_pid=$!
wait "$agent_pid"
