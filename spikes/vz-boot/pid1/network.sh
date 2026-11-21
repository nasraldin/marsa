test "$(busybox grep -c '^nameserver 198.18.0.54$' /etc/resolv.conf)" = 1
test "$(/marsa-test-docker info --format '{{.HTTPProxy}}')" = 'http://198.18.0.99:3128'
busybox grep -q 'http://198.18.0.98:3128' /etc/docker/daemon.json
/sbin/ip addr add 198.18.0.10/32 dev lo
/marsa-network-registry serve /marsa-network-registry.yml > /run/marsa/registry.log 2>&1 &
registry_pid=$!
trap 'busybox kill -TERM "$registry_pid" || :; wait "$registry_pid" || :' EXIT
image=registry.marsa.test:5443/marsa/test:verified
/marsa-test-docker import --platform linux/arm64 /marsa-network-alpine.tar.gz "$image"
/marsa-test-docker push "$image"
/marsa-test-docker image rm "$image"
/marsa-test-docker pull "$image"
/marsa-test-docker run --rm --add-host host.docker.internal:host-gateway "$image" /bin/sh -ec '
    grep -q "198.18.0.10.*host.docker.internal" /etc/hosts
    grep -q "^nameserver 198.18.0.54$" /etc/resolv.conf
'
printf 'MARSA_TLS_REGISTRY_AND_DNS_OK\n'
