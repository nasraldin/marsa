<<<<<<< HEAD
//! Verify devices and NAT using the packaged guest on an entitled Apple silicon host.

use std::{env, fs, path::Path, time::Duration};

use marsa_proto::guest::CONTROL_PORT;
use marsa_vm::{Hypervisor, ShareSpec, Vm, VmError, VmState, VsockStream};
use marsa_vm_vz::VzHypervisor;
use tokio::time::{sleep, timeout};

use super::{LIMIT, handshake, spec, transition};
=======
//! Verify virtiofs, memory ballooning and power controls using the packaged guest on an entitled Apple silicon host.

use std::{env, fs, path::Path, time::Duration};

use marsa_vm::{Hypervisor, ShareSpec, VmError, VmState};
use marsa_vm_vz::VzHypervisor;
use tokio::time::timeout;

use super::{LIMIT, connect_control, console_with, handshake, spec, transition};
>>>>>>> origin/main

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
<<<<<<< HEAD
=======
    drop(stream);
    let mut stream = connect_control(&*vm, &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
>>>>>>> origin/main
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
    assert!(vm.network_info().is_none());
}
<<<<<<< HEAD

async fn connect_control(vm: &dyn Vm, console: &Path) -> VsockStream {
    timeout(LIMIT, async {
        loop {
            match vm.connect_vsock(CONTROL_PORT).await {
                Ok(stream) => break stream,
                Err(VmError::GuestPortUnavailable { .. }) => sleep(Duration::from_millis(25)).await,
                Err(error) => panic!("{error}\n{}", fs::read_to_string(console).unwrap()),
            }
        }
    })
    .await
    .unwrap_or_else(|error| panic!("{error}\n{}", fs::read_to_string(console).unwrap()))
}

async fn console_with(console: &Path, marker: &str) -> String {
    timeout(LIMIT, async {
        loop {
            let log = fs::read_to_string(console).unwrap();
            if log.contains(marker) {
                break log;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "console did not contain {marker:?}: {error}\n{}",
            fs::read_to_string(console).unwrap()
        )
    })
}

#[tokio::test]
#[ignore = "requires virtualization entitlement, native NAT and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_guest_nat_address_gateway_and_https() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let cmdline = concat!(
        "console=hvc0 root=/dev/vda rootfstype=erofs ro panic=-1 init=/bin/sh -- -c \"",
        "exec </dev/hvc0 >/dev/hvc0 2>&1; (",
        "while [ ! -S /var/run/docker.sock ]; do /bin/busybox sleep 1; done; ",
        "/sbin/ip -4 -o addr show dev eth0; ",
        "/sbin/ip -4 route show default dev eth0; echo MARSA_NET_CONFIG_CAPTURED; ",
        "/bin/busybox wget -T 5 -O /dev/null https://example.com && echo MARSA_NAT_HTTPS_OK; ",
        ") & exec /usr/bin/marsa-init\"",
    );
    let spec = spec(&guest, scratch.path(), cmdline);
    let console = spec.console_log.clone();
    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(vm.network_info().is_none());
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    let mut stream = connect_control(&*vm, &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    let network = timeout(LIMIT, async {
        loop {
            if let Some(facts) = vm.network_info() {
                break facts;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the backend did not get network facts");
    let log = console_with(&console, "MARSA_NET_CONFIG_CAPTURED\r\n").await;
    assert!(
        log.lines()
            .any(|line| line.contains(&format!("eth0    inet {}/", network.address))),
        "{log}"
    );
    let gateway = network.gateway.expect("NAT default gateway");
    let gateway_words = ["default", "via", &gateway.to_string()];
    assert!(
        log.lines()
            .any(|line| line.split_ascii_whitespace().take(3).eq(gateway_words)),
        "{log}"
    );
    println!(
        "guest DHCP address {} via {gateway} matches the backend snapshot",
        network.address
    );
    console_with(&console, "MARSA_NAT_HTTPS_OK\r\n").await;
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    assert!(vm.network_info().is_none());
}
=======
>>>>>>> origin/main
