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
