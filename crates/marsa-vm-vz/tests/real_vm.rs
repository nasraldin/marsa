//! Entitled system tests using the actual packaged ARM64 guest on Apple silicon.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "real_vm/devices.rs"]
mod devices;

#[cfg(feature = "native-ipv4-system-test")]
#[path = "real_vm/native_ipv4.rs"]
mod native_ipv4;

#[cfg(feature = "ssh-transport-system-test")]
#[path = "real_vm/ssh_transport.rs"]
mod ssh_transport;

#[cfg(feature = "userspace-system-test")]
#[path = "real_vm/user_network.rs"]
mod user_network;

use std::{
    env,
    fs::{self, File},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use marsa_proto::guest::{
    CONTROL_PORT, DOCKER_API_PORT, decode_guest, encode_host, frame_length, negotiate,
    v1::{
        CommandStatus, EngineHealthState, GuestEnvelope, HelloAck, HostEnvelope, SetTime,
        guest_envelope::Body as GuestBody, host_envelope::Body as HostBody,
    },
};
use marsa_vm::{
    DiskSpec, Hypervisor, NetworkSpec, VmError, VmEvent, VmEventStream, VmExitCause, VmSpec,
    VmState, VsockStream,
};
use marsa_vm_vz::VzHypervisor;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{sleep, timeout},
};
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
        Some(VmEvent::UnexpectedExit { cause: VmExitCause::GuestPowerOff, detail }) if detail.contains("without a stop request"),
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

const CHECK_ROOT: &str = concat!(
    "console=hvc0 root=/dev/vda rootfstype=erofs ro marsa.interface=none panic=-1 init=/bin/sh -- -c \"",
    "exec </dev/hvc0 >/dev/hvc0 2>&1; ",
    "/bin/busybox test -b /dev/vda || exit 1; ",
    "/bin/busybox mount -t sysfs -o ro,nodev,noexec,nosuid sysfs /sys || exit 1; ",
    "read -r ro </sys/class/block/vda/ro; /bin/busybox umount /sys || exit 1; ",
    "test $ro = 1 || exit 1; echo MARSA_ROOT_DEVICE_RO; ",
    "if /bin/busybox dd if=/dev/zero of=/dev/vda bs=1 count=1 conv=notrunc; then exit 1; fi; ",
    "if /bin/busybox touch /.marsa-vz-write-test; then exit 1; fi; ",
    "echo MARSA_ROOT_WRITE_REJECTED; exec /usr/bin/marsa-init\"",
);

async fn read_guest(stream: &mut VsockStream) -> GuestEnvelope {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await.unwrap();
    let mut payload = vec![0; frame_length(header).unwrap()];
    stream.read_exact(&mut payload).await.unwrap();
    decode_guest(&payload).unwrap()
}

async fn send_host(stream: &mut VsockStream, request_id: u64, body: HostBody) {
    let frame = encode_host(&HostEnvelope {
        request_id,
        body: Some(body),
    })
    .unwrap();
    stream.write_all(&frame).await.unwrap();
}

async fn handshake(stream: &mut VsockStream) {
    negotiate_and_set_time(stream, false).await;
}

async fn negotiate_and_set_time(stream: &mut VsockStream, defer_engine_start: bool) {
    let greeting = read_guest(stream).await;
    assert_eq!(greeting.request_id, 0);
    let Some(GuestBody::Hello(hello)) = greeting.body else {
        panic!("expected the packaged agent's greeting: {greeting:?}");
    };
    assert_eq!(hello.image_version, env!("CARGO_PKG_VERSION"));
    let protocol = negotiate(hello.protocol.unwrap()).unwrap();
    send_host(
        stream,
        0,
        HostBody::HelloAck(HelloAck {
            protocol: Some(protocol),
            defer_engine_start,
        }),
    )
    .await;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    send_host(
        stream,
        1,
        HostBody::SetTime(SetTime {
            unix_seconds: i64::try_from(now.as_secs()).unwrap(),
            nanoseconds: now.subsec_nanos(),
        }),
    )
    .await;

    let mut time_acknowledged = false;
    let mut engine_ready = defer_engine_start;
    while !time_acknowledged || !engine_ready {
        let event = read_guest(stream).await;
        match event.body {
            Some(GuestBody::CommandResult(result)) => {
                assert_eq!(event.request_id, 1);
                assert_eq!(result.status, CommandStatus::Ok as i32, "{result:?}");
                time_acknowledged = true;
            }
            Some(GuestBody::EngineHealth(health)) => {
                engine_ready |= health.state == EngineHealthState::Ready as i32;
            }
            _ => {}
        }
    }
}

async fn connect_control(vm: &dyn marsa_vm::Vm, console: &Path) -> VsockStream {
    let connection = timeout(LIMIT, async {
        loop {
            match vm.connect_vsock(CONTROL_PORT).await {
                Ok(stream) => break stream,
                Err(VmError::GuestPortUnavailable { port }) if port == CONTROL_PORT => {
                    sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("connect to port {CONTROL_PORT}: {error:?}"),
            }
        }
    })
    .await;
    connection.unwrap_or_else(|error| {
        panic!(
            "agent did not listen within {LIMIT:?}: {error}\n{}",
            fs::read_to_string(console).unwrap()
        );
    })
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_guest_with_no_upstream_link_keeps_local_docker_ready() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let cmdline = "console=hvc0 root=/dev/vda rootfstype=erofs ro init=/usr/bin/marsa-init marsa.interface=missing0 panic=-1";
    let spec = spec(&guest, scratch.path(), cmdline);
    let console = spec.console_log.clone();
    let mut vm = VzHypervisor.create(spec).await.unwrap();
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();
    let mut stream = connect_control(vm.as_ref(), &console).await;
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();

    timeout(LIMIT, async {
        let mut docker = vm.connect_vsock(DOCKER_API_PORT).await.unwrap();
        docker
            .write_all(b"GET /_ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut status = [0; 12];
        docker.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"HTTP/1.1 200");
    })
    .await
    .unwrap();

    let log = fs::read_to_string(console).unwrap();
    assert!(
        log.contains("upstream interface missing0 is offline"),
        "{log}"
    );
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn packaged_agent_handshake_read_only_root_and_full_console() {
    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let mut spec = spec(&guest, scratch.path(), CHECK_ROOT);
    let original_root = spec.root_disk.path.clone();
    let test_root = scratch.path().join("rootfs.img");
    fs::copy(&original_root, &test_root).unwrap();
    let root_digest = Sha256::digest(fs::read(&test_root).unwrap());
    spec.root_disk.path = test_root.clone();
    let console = spec.console_log.clone();
    let mut vm = VzHypervisor.create(spec).await.unwrap();
    assert!(matches!(
        vm.connect_vsock(CONTROL_PORT).await,
        Err(VmError::InvalidState {
            state: VmState::Created,
            ..
        })
    ));
    timeout(LIMIT, vm.start()).await.unwrap().unwrap();

    let mut stream = connect_control(vm.as_ref(), &console).await;
    let unused = timeout(LIMIT, vm.connect_vsock(5_999)).await.unwrap();
    assert!(
        matches!(unused, Err(VmError::GuestPortUnavailable { port: 5_999 })),
        "{:?}",
        unused.err()
    );
    timeout(LIMIT, handshake(&mut stream)).await.unwrap();
    timeout(LIMIT, vm.stop(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    drop(stream);

    let log = fs::read_to_string(console).unwrap();
    let mut rest = log.as_str();
    for marker in [
        "virtio_blk ",
        "Mounted root (erofs filesystem) readonly",
        "Run /bin/sh as init process",
        "MARSA_ROOT_DEVICE_RO\r\n",
        "dd: can't open '/dev/vda':",
        "touch: /.marsa-vz-write-test: Read-only file system",
        "MARSA_ROOT_WRITE_REJECTED\r\n",
        "marsa-init: replaying early kernel boot log",
        "Booting Linux on physical CPU",
        "Kernel command line:",
        "marsa-init: end of early kernel boot log",
        "marsa-init stage=mount_system",
        "marsa-init stage=containerd",
        "marsa-init stage=dockerd",
        "API listen on /var/run/docker.sock",
    ] {
        let Some((_, tail)) = rest.split_once(marker) else {
            panic!("console missing ordered marker {marker:?}:\n{log}");
        };
        rest = tail;
    }
    assert_eq!(
        log.matches("Booting Linux on physical CPU").count(),
        1,
        "{log}"
    );
    assert_eq!(Sha256::digest(fs::read(test_root).unwrap()), root_digest);
}

#[tokio::test]
#[ignore = "requires virtualization entitlement and MARSA_VZ_GUEST_DIR on Apple silicon"]
async fn guest_refuses_partition_table_and_last_sector_without_writing() {
    use std::io::Write;
    use std::os::unix::fs::FileExt;

    let guest = env::var_os("MARSA_VZ_GUEST_DIR").expect("set MARSA_VZ_GUEST_DIR");
    let guest = Path::new(&guest).canonicalize().unwrap();
    for partitioned in [false, true] {
        let scratch = tempfile::tempdir().unwrap();
        let config = spec(&guest, scratch.path(), BOOT);
        let path = config.data_disk.path.clone();
        let console = config.console_log.clone();
        let mut disk = fs::OpenOptions::new().write(true).open(&path).unwrap();
        // Include a partial final scan chunk and a complete final logical sector.
        let length = 16 * 1024 * 1024 + 512;
        disk.set_len(length).unwrap();
        if partitioned {
            let mut mbr = [0u8; 512];
            mbr[450] = 0x83;
            mbr[454..458].copy_from_slice(&2048u32.to_le_bytes());
            mbr[458..462].copy_from_slice(&4096u32.to_le_bytes());
            mbr[510..512].copy_from_slice(&[0x55, 0xaa]);
            disk.write_all(&mbr).unwrap();
        } else {
            disk.write_all_at(b"X", length - 1).unwrap();
        }
        disk.sync_all().unwrap();
        drop(disk);
        let before = Sha256::digest(fs::read(&path).unwrap());
        let mut vm = VzHypervisor.create(config).await.unwrap();
        timeout(LIMIT, vm.start()).await.unwrap().unwrap();
        let log = console_with(
            &console,
            "neither blank nor an unpartitioned ext4 filesystem",
        )
        .await;
        assert!(!log.contains("data disk format started"));
        if !partitioned {
            assert!(log.contains("blank=false"), "{log}");
        }
        drop(vm);
        assert_eq!(Sha256::digest(fs::read(path).unwrap()), before);
    }
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

#[cfg(any(feature = "native-ipv4-system-test", feature = "userspace-system-test"))]
async fn network_with_gateway(vm: &dyn marsa_vm::Vm, console: &Path) -> marsa_vm::NetworkInfo {
    timeout(LIMIT, async {
        loop {
            if let Some(facts) = vm.network_info().filter(|facts| facts.gateway.is_some()) {
                break facts;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "the backend did not report a guest address and default gateway: {error}; last snapshot: {:?}\n{}",
            vm.network_info(),
            fs::read_to_string(console).unwrap(),
        )
    })
}
