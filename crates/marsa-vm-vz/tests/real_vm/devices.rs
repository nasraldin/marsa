//! Verify virtiofs, memory ballooning and power controls using the packaged guest on an entitled Apple silicon host.

use std::{env, fs, path::Path, time::Duration};

use marsa_vm::{Hypervisor, ShareSpec, VmError, VmState};
use marsa_vm_vz::VzHypervisor;
use tokio::time::timeout;

use super::{LIMIT, connect_control, console_with, handshake, spec, transition};

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_guest_share_balloon_and_pause_resume() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    // Use a pathname with characters the boot arguments must encode.
    let shares = tempfile::tempdir_in("/private/tmp").unwrap();
    let shared = shares.path().join("Project % files");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("proof"), b"from-host-virtiofs").unwrap();

    // The child verifies the mounted path after setup. The parent becomes packaged PID 1.
    let cmdline = format!(
        "console=hvc0 root=/dev/vda rootfstype=erofs ro marsa.interface=none panic=-1 init=/bin/sh -- -c \"\
         exec </dev/hvc0 >/dev/hvc0 2>&1; (\
         while [ ! -S /var/run/docker.sock ]; do /bin/busybox sleep 1; done; \
         /bin/busybox cat '{path}/proof' && echo && \
         if /bin/busybox touch '{path}/denied'; then exit 1; fi && \
         echo MARSA_SHARE_RO; \
         while ! /bin/busybox grep -q '^balloon_inflate [1-9]' /proc/vmstat; do /bin/busybox sleep 1; done; \
         /bin/busybox grep '^balloon_inflate ' /proc/vmstat; echo MARSA_BALLOON_INFLATED; \
         ) & exec /usr/bin/marsa-init\"",
        path = shared.display(),
    );
    let mut spec = spec(&guest, scratch.path(), &cmdline);
    spec.shares.push(ShareSpec {
        tag: "project".into(),
        host_path: shared.clone(),
        read_only: true,
    });
    let console = spec.console_log.clone();
    let capabilities = VzHypervisor.capabilities();
    assert!(capabilities.memory_balloon && capabilities.pause_resume);
    assert!(!capabilities.dynamic_shares && !capabilities.userspace_network);

    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(vm.network_info().is_none());
    assert!(matches!(
        vm.pause().await,
        Err(VmError::InvalidState {
            state: VmState::Created,
            ..
        })
    ));
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;

    let mut stream = connect_control(&*vm, &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    let log = console_with(&console, "MARSA_SHARE_RO\r\n").await;
    assert!(log.contains("from-host-virtiofs\r\n"), "{log}");
    assert!(!shared.join("denied").exists());

    assert!(matches!(
        vm.set_memory_target(1).await,
        Err(VmError::InvalidSpec {
            field: "memory_target",
            ..
        })
    ));
    vm.set_memory_target(896 * 1024 * 1024).await.unwrap();
    let balloon_log = console_with(&console, "MARSA_BALLOON_INFLATED\r\n").await;
    assert!(
        balloon_log
            .lines()
            .any(|line| line.starts_with("balloon_inflate "))
    );
    vm.set_memory_target(1024 * 1024 * 1024).await.unwrap();

    timeout(LIMIT, vm.pause()).await.unwrap().unwrap();
    transition(&mut events, VmState::Running, VmState::Paused).await;
    assert_eq!(vm.state(), VmState::Paused);
    timeout(LIMIT, vm.resume()).await.unwrap().unwrap();
    transition(&mut events, VmState::Paused, VmState::Running).await;
    assert_eq!(vm.state(), VmState::Running);
    drop(stream);
    let mut stream = connect_control(&*vm, &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
    assert!(vm.network_info().is_none());
}
