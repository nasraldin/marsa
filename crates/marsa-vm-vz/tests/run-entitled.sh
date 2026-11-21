#!/usr/bin/env bash
# Build and run the production VZ system tests from the repository root.
set -euo pipefail

export MARSA_VZ_GUEST_DIR="${1:-"$PWD/target/guest"}"
test_executable=$(
    MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsa-vm-vz \
        --test real_vm --no-run --message-format=json |
    python3 -c '
import json
import sys

tests = [
    record["executable"]
    for record in map(json.loads, sys.stdin)
    if record.get("reason") == "compiler-artifact"
    and record["target"]["name"] == "real_vm"
    and record.get("executable")
]
if len(tests) != 1:
    sys.exit("expected a single native system test executable")
print(tests[0])
'
)
codesign --force --sign - --timestamp=none \
    --entitlements spikes/vz-boot/entitlement.plist "$test_executable"
codesign --verify --strict "$test_executable"
exec "$test_executable" --ignored --nocapture --test-threads=1
