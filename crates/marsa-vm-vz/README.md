# Run the native VZ system tests

The ignored tests boot the packaged Linux kernel and EROFS image on a physical
Apple silicon Mac. Run them with macOS 27 or later from the repository root.
Build the guest with `cargo xtask guest all`, then run the same command as Tier 2.

```sh
bash crates/marsa-vm-vz/tests/run-entitled.sh target/guest
```

The script builds and ad hoc signs the test executable with the virtualization
entitlement. The tests use disposable sparse data disks and check the native VM
lifecycle, negotiate the guest agent protocol, and wait for Docker API readiness.
They attempt a raw device write and a filesystem write against a disposable copy
of the root image. The console assertions cover boot, the failed writes, PID 1
and Docker startup in order. PID 1 replays the messages Linux retained before
`hvc0` registration in a marked section of the log.

<<<<<<< HEAD
The native-device test verifies read-only access at the original host
share path, a guest HTTPS request over NAT, the reported address and
gateway, the requested balloon size, and the pause and resume state events.
It reaches `example.com` from the guest to exercise outbound networking.
=======
The regular suite also checks same-path read-only virtiofs, guest balloon
inflation, and pause and resume state changes. Set `MARSA_VZ_NETWORK_HELPER`
to the absolute path of the [built helper](../../network-helper/README.md)
to include its DHCP facts, lifecycle, and TLS-verified registry pull.

Qualify native NAT separately on an IPv4-capable test network.

```sh
MARSA_ENTITLED_NATIVE_IPV4=1 bash crates/marsa-vm-vz/tests/run-entitled.sh target/guest
```

The native test requires the backend's IPv4 address and default gateway to match
the guest kernel. It resolves and connects to a specific registry IPv4 address
through native NAT, verifies the registry's TLS certificate, and checks the
HTTPS response.
>>>>>>> origin/main
