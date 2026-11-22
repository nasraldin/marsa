# SSH agent transport fixture

This fixture boots the pinned ARM64 kernel with a disposable initramfs under
the entitled Apple Virtualization backend. It does not mount host directories
or configure the guest network interface. It uses two Linux cgroups with
distinct, Docker-shaped IDs and gives both test processes access to the same
Unix socket. The guest compares the kernel's captured peer PID and pidfd with
the opener's cgroup and checks liveness before and after reading its identity.

The host test starts a fresh OpenSSH agent with a single disposable Ed25519
key, a temporary home and an explicit socket path. It clears inherited agent
and keychain-related environment settings. The guest verifies an actual
signature from that agent. The fixture bounds the protocol frames and only
forwards its identity and signature requests. A remove-all request receives
failure, and the agent still reports exactly one key. Closing the native host
connection causes the active Unix client to observe EOF and a fresh open to
fail. The guest's original unsolicited connection to the host does not
create an agent session.

Use the existing packaged guest kernel and root disk plus the verified Alpine
tarball from `cargo xtask guest all`. Go 1.27.4 compiles the static Linux probe
without fetching modules. From the repository root, run:

```sh
fixture=$(mktemp -d)
sh crates/marsa-vm-vz/tests/real_vm/ssh_fixture/prepare.sh \
  "$PWD/target/guest/alpine-minirootfs-3.24.4-aarch64.tar.gz" "$fixture"

MARSA_ENTITLED_SSH_TRANSPORT=1 \
MARSA_VZ_SSH_INITRAMFS="$fixture/initramfs.gz" \
  bash crates/marsa-vm-vz/tests/run-entitled.sh "$PWD/target/guest"
```

The fixture uses synthetic cgroups and contains no Docker grant or production
relay. M4-17 must verify the real engine's mapping from an approved
container's immutable ID to its cgroup. A Docker client can inherit a socket
mount through `--volumes-from`, so mount inspection alone cannot establish
socket access. Guest root and trusted Docker-socket holders can control
processes in the approved workload. An approved process can also deliberately
relay its own signing access. These capabilities are outside this fixture's
ordinary container isolation check. [Pinned Linux peer credential code](https://github.com/gregkh/linux/blob/v7.3.5/net/core/sock.c) · [Pinned Docker mount inheritance](https://github.com/moby/moby/blob/docker-v29.10.0/daemon/volumes.go)
