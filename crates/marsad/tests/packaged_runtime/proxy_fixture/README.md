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
