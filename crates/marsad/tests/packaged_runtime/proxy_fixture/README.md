# Guest-local proxy traffic fixture

This fixture contains an unmodified Docker engine and BuildKit. Its guest
loopback hosts an OCI registry over TLS and an HTTP proxy that accepts
CONNECT only for that registry. The registry has two distinct OCI repositories
and never listens on a host or VM network interface. The fresh certificate
authority is trusted only inside this disposable guest. The fixture logs fixed
route and manifest events without recording traffic or proxy credentials.

Use the pinned Go compiler, the packaged guest output and its verified Alpine
tarball from `cargo xtask guest all`. The preparation script runs the pinned
EROFS formatter under QEMU software emulation with no network device. It uses
an empty disposable directory. The resulting fixture contains a short-lived
guest server key and must stay private.

```sh
fixture=$(mktemp -d)
sh crates/marsad/tests/packaged_runtime/proxy_fixture/prepare.sh \
  "$PWD/target/guest" "$PWD/target/guest/alpine-minirootfs-3.24.4-aarch64.tar.gz" "$fixture"

MARSA_VZ_PROXY_FIXTURE_DIR="$fixture/guest-fixture" \
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad --test packaged_runtime \
  daemon_proxy::approved_activation_carries_registry_and_buildkit_requests_through_guest_proxy \
  -- --exact --ignored --nocapture --test-threads=1
```

Set `MARSA_VZ_NETWORK_HELPER` to the built helper. Put the pinned Docker CLI
and Buildx binary on `PATH`. Tier 2 prepares this fixture alongside the shipping guest. The dedicated
invocation above runs the same traffic check on a development Mac.

The test first saves a draft and declines approval while the engine is
running. Both operations leave the live proxy unset. After approval it reads
the running proxy, checks the same persistent disk, pulls the first image and
builds from the second using Buildx's default Docker driver. The CONNECT
events and distinct registry manifests establish the route. The BuildKit
output contains the marker from the second image.

## Intercepting proxy and macOS Keychain

Run this case in an interactive, disposable macOS account with virtualization
available. Its user trust domain must be dedicated to the test. Set
`MARSA_TEST_DISPOSABLE_TRUST_USER` to that account's login name. macOS may
ask the operator to approve adding and removing “Marsa Disposable Proxy Test
CA” in that account. Discard the account after qualification, including if
an assertion fails before revocation.

```sh
fixture=$(mktemp -d)
sh crates/marsad/tests/packaged_runtime/proxy_fixture/prepare.sh \
  "$PWD/target/guest" "$PWD/target/guest/alpine-minirootfs-3.24.4-aarch64.tar.gz" \
  "$fixture" --intercept
MARSA_VZ_INTERCEPT_FIXTURE_DIR="$fixture/guest-fixture" \
MARSA_TEST_DISPOSABLE_TRUST_DRIVER="$PWD/crates/marsad/tests/packaged_runtime/proxy_fixture/disposable_trust.sh" \
MACOSX_DEPLOYMENT_TARGET=27.0 cargo test --locked -p marsad --test packaged_runtime \
  intercept_trust:: -- --ignored --nocapture --test-threads=1
```

The fixture starts with the shipping guest trust bundle. Two guest-loopback
proxy ports accept CONNECT for the synthetic registry only, terminate TLS,
and forward OCI requests to a separate loopback HTTP origin. The test checks
rejection before trust, a successful pull through the second approved port on
the same data disk, and rejection of a fresh repository after revocation and
restart. Keep the scratch directory private and discard it after the test.

With a proxy scratch directory already prepared from this checkout, reuse its
verified formatter and guest endpoints binary. The last argument must name a
new directory. Set `MARSA_VZ_INTERCEPT_FIXTURE_DIR` to its `guest-fixture`
subdirectory for the same test.

```sh
sh crates/marsad/tests/packaged_runtime/proxy_fixture/prepare_intercept.sh \
  "$PWD/target/guest" "$fixture" "$fixture/intercept"
```
