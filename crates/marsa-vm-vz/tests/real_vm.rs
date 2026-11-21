//! Entitled system tests using the actual packaged ARM64 guest on Apple silicon.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::{
    env,
    fs::{self, File},
    path::Path,
    time::Duration,
};

use marsa_vm::{DiskSpec, Hypervisor, NetworkSpec, VmEvent, VmEventStream, VmSpec, VmState};
use marsa_vm_vz::VzHypervisor;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;

const LIMIT: Duration = Duration::from_secs(15);
const BOOT: &str = "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init marsa.interface=none panic=-1";
const GUEST_POWEROFF: &str = concat!(
    "console=hvc0 root=/dev/vda rootfstype=erofs ro panic=-1 init=/bin/sh -- -c \"",
    "/bin/busybox sleep 1; /bin/busybox poweroff -f\"",
);

fn spec(guest: &Path, scratch: &Path, cmdline: &str) -> VmSpec {
    let data = scratch.join("data.img");
    File::create(&data)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    VmSpec {
        cpus: 2,
        memory_bytes: 1024 * 1024 * 1024,
        kernel: guest.join("Image"),
        initramfs: None,
        cmdline: cmdline.into(),
        root_disk: DiskSpec {
            path: guest.join("rootfs.img"),
            read_only: true,
        },
        data_disk: DiskSpec {
            path: data,
            read_only: false,
        },
        shares: Vec::new(),
        network: NetworkSpec::Nat,
        console_log: scratch.join("console.log"),
    }
}

async fn transition(events: &mut VmEventStream, previous: VmState, current: VmState) {
    assert_eq!(
        timeout(LIMIT, events.next()).await.unwrap(),
        Some(VmEvent::StateChanged { previous, current }),
    );
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_init_boot_and_native_lifecycle_events() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();

    let data = tempfile::tempdir().unwrap();
    let spec_boot = spec(&guest, data.path(), BOOT);
    let console = spec_boot.console_log.clone();
    let mut vm = VzHypervisor.create(spec_boot).await.unwrap();
    assert_eq!(vm.state(), VmState::Created);
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;

    timeout(LIMIT, async {
        loop {
            let text = fs::read_to_string(&console).unwrap();
            if text.contains("marsa-init stage=data_disk") {
                assert!(
                    text.contains("Mounted root (erofs filesystem) readonly"),
                    "{text}"
                );
                assert!(
                    text.contains("Run /usr/bin/marsa-init as init process"),
                    "{text}"
                );
                assert!(text.contains("marsa-init stage=mount_system"), "{text}");
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the packaged PID 1 did not reach data disk initialization");

    timeout(LIMIT, vm.stop(Duration::from_millis(50)))
        .await
        .unwrap()
        .unwrap();
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
    assert_eq!(vm.state(), VmState::Stopped);
    drop(vm);

    let poweroff = tempfile::tempdir().unwrap();
    let mut vm = VzHypervisor
        .create(spec(&guest, poweroff.path(), GUEST_POWEROFF))
        .await
        .unwrap();
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;
    transition(&mut events, VmState::Running, VmState::Failed).await;
    assert!(matches!(
        timeout(LIMIT, events.next()).await.unwrap(),
        Some(VmEvent::UnexpectedExit { detail }) if detail.contains("without a stop request"),
    ));
    assert_eq!(vm.state(), VmState::Failed);
    drop(vm);

    let cleanup = tempfile::tempdir().unwrap();
    let mut vm = VzHypervisor
        .create(spec(&guest, cleanup.path(), BOOT))
        .await
        .unwrap();
    let mut events = vm.events();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    transition(&mut events, VmState::Created, VmState::Starting).await;
    transition(&mut events, VmState::Starting, VmState::Running).await;
    drop(vm);
    transition(&mut events, VmState::Running, VmState::Stopping).await;
    transition(&mut events, VmState::Stopping, VmState::Stopped).await;
}
