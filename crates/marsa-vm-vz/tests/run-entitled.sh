#!/usr/bin/env bash
# Build and run the production VZ or daemon network test from the repository root.
set -euo pipefail

export MARSA_VZ_GUEST_DIR="${1:-"$PWD/target/guest"}"
if (( $# > 0 )); then
    shift
fi
package=marsa-vm-vz
test_name=real_vm
features=()
if [[ ${MARSA_ENTITLED_DAEMON_NETWORK:-0} == 1 ]]; then
    package=marsad
    test_name=network_boot
elif [[ ${MARSA_ENTITLED_SSH_TRANSPORT:-0} == 1 ]]; then
    features=(--features ssh-transport-system-test)
    set -- ssh_transport::host_initiated_signing_stays_with_the_approved_socket_peer --exact "$@"
elif [[ ${MARSA_ENTITLED_NATIVE_IPV4:-0} == 1 ]]; then
    features=(--features native-ipv4-system-test)
    set -- native_ipv4::packaged_guest_nat_address_gateway_and_https --exact "$@"
elif [[ -n "${MARSA_VZ_NETWORK_HELPER:-}" ]]; then
    features=(--features userspace-system-test)
fi
test_executable=$(
    MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p "$package" \
        --test "$test_name" --no-run --message-format=json ${features[@]+"${features[@]}"} |
    python3 -c '
import json
import sys

tests = [
    record["executable"]
    for record in map(json.loads, sys.stdin)
    if record.get("reason") == "compiler-artifact"
    and record["target"]["name"] == sys.argv[1]
    and record.get("executable")
]
if len(tests) != 1:
    sys.exit("expected a single native system test executable")
print(tests[0])
' "$test_name"
)
codesign --force --sign - --timestamp=none \
    --entitlements spikes/vz-boot/entitlement.plist "$test_executable"
codesign --verify --strict "$test_executable"
exec "$test_executable" --ignored --nocapture --test-threads=1 "$@"
